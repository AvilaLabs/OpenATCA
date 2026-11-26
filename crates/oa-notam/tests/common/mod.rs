//! Fixtures shared by the oa-notam tests. Every identifier here is made up; none comes from the
//! real feed.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_notam::*;
use oa_xml::Parser;

pub const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><message:AIXMBasicMessage xmlns:message="http://www.aixm.aero/schema/5.1/message" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:aixm="http://www.aixm.aero/schema/5.1" xmlns:event="http://www.aixm.aero/schema/5.1/event" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:fns="urn:us.gov.dot.faa.aim.fns" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:html="http://www.w3.org/1999/xhtml" xmlns:fnse="http://www.aixm.aero/schema/5.1/extensions/FAA/FNSE" xsi:schemaLocation="http://www.aixm.aero/schema/5.1/message https://example.invalid/x.xsd" gml:id="M1">"#;
pub const TAIL: &str = "</message:AIXMBasicMessage>";

/// 2030-03-05T10:00:00Z, 2030-03-06T18:30:00Z and 2030-03-05T09:00:00Z in Unix ms.
pub const START: i64 = 1_898_935_200_000;
pub const END: i64 = 1_899_052_200_000;
pub const ISSUED: i64 = 1_898_931_600_000;

pub const EVENT_ID: [u8; 16] = [
    0x0a, 0x1b, 0x2c, 0x3d, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0xe0, 0x01,
];

pub const EVENT: &str = r#"<message:hasMember><event:Event gml:id="E1"><gml:identifier codeSpace="urn:uuid:">0a1b2c3d-0000-4000-8000-00000000e001</gml:identifier><gml:boundedBy xsi:nil="true"/><event:timeSlice><event:EventTimeSlice gml:id="ET1"><gml:validTime><gml:TimePeriod gml:id="EP1"><gml:beginPosition>2030-03-05T10:00:00.000Z</gml:beginPosition><gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition></gml:TimePeriod></gml:validTime><aixm:interpretation>BASELINE</aixm:interpretation><aixm:sequenceNumber>1</aixm:sequenceNumber><aixm:correctionNumber>0</aixm:correctionNumber><event:scenario>123</event:scenario><event:textNOTAM><event:NOTAM gml:id="N1"><event:series>Z</event:series><event:number>42</event:number><event:year>2030</event:year><event:type>N</event:type><event:issued>2030-03-05T09:00:00.000Z</event:issued><event:affectedFIR>ZQQ</event:affectedFIR><event:selectionCode>QXXXX</event:selectionCode><event:minimumFL>000</event:minimumFL><event:maximumFL>999</event:maximumFL><event:coordinates>3012N09045W</event:coordinates><event:radius>5</event:radius><event:location>QQA</event:location><event:effectiveStart>203003051000</event:effectiveStart><event:effectiveEnd>203003061830</event:effectiveEnd><event:text>RWY 09/27 CLSD</event:text><event:translation><event:NOTAMTranslation gml:id="NT1"><event:type>LOCAL_FORMAT</event:type><event:simpleText>!QQA 03/042 QQA RWY 09/27 CLSD</event:simpleText></event:NOTAMTranslation></event:translation></event:NOTAM></event:textNOTAM><event:extension><fnse:EventExtension gml:id="X1"><fnse:classification>DOM</fnse:classification><fnse:accountId>QQA</fnse:accountId><fnse:xovernotamID>Z0042/30</fnse:xovernotamID><fnse:lastUpdated>2030-03-05T09:00:00.000Z</fnse:lastUpdated><fnse:icaoLocation>KQQA</fnse:icaoLocation></fnse:EventExtension></event:extension></event:EventTimeSlice></event:timeSlice></event:Event></message:hasMember>"#;

pub const INSTANT: &str = r#"<gml:validTime><gml:TimeInstant gml:id="I"><gml:timePosition>2030-03-05T09:00:00.000Z</gml:timePosition></gml:TimeInstant></gml:validTime>"#;
pub const PERIOD: &str = r#"<gml:validTime><gml:TimePeriod gml:id="P"><gml:beginPosition>2030-03-05T10:00:00.000Z</gml:beginPosition><gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition></gml:TimePeriod></gml:validTime>"#;

/// A feature member with one time slice.
pub fn member(feature: &str, valid: &str, inner: &str) -> String {
    format!(
        "<message:hasMember><aixm:{feature} gml:id=\"F\"><gml:identifier codeSpace=\"urn:uuid:\">0a1b2c3d-0000-4000-8000-00000000f001</gml:identifier><aixm:timeSlice><aixm:{feature}TimeSlice gml:id=\"S\">{valid}<aixm:interpretation>SNAPSHOT</aixm:interpretation>{inner}</aixm:{feature}TimeSlice></aixm:timeSlice></aixm:{feature}></message:hasMember>"
    )
}

pub const SRS: &str = r#"srsDimension="2" srsName="urn:ogc:def:crs:EPSG::4326""#;
pub const RING: &str = "30.10 -90.10 30.10 -90.20 30.20 -90.20 30.20 -90.10 30.10 -90.10";

pub fn extent(ring: &str) -> String {
    format!(
        "<aixm:extent><aixm:ElevatedSurface gml:id=\"S\" {SRS}><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>{ring}</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></aixm:ElevatedSurface></aixm:extent>"
    )
}

pub fn airport() -> String {
    member(
        "AirportHeliport",
        INSTANT,
        &format!(
            "<aixm:designator>QQA</aixm:designator><aixm:name>TEST FIELD</aixm:name><aixm:locationIndicatorICAO>KQQA</aixm:locationIndicatorICAO><aixm:ARP><aixm:ElevatedPoint gml:id=\"P\" {SRS}><gml:pos>30.2 -90.75</gml:pos></aixm:ElevatedPoint></aixm:ARP>"
        ),
    )
}
pub fn runway() -> String {
    member(
        "Runway",
        INSTANT,
        "<aixm:designator>09L/27R</aixm:designator><aixm:associatedAirportHeliport xlink:href=\"#A\"/>",
    )
}
pub fn direction() -> String {
    member(
        "RunwayDirection",
        INSTANT,
        "<aixm:designator>27R</aixm:designator><aixm:usedRunway xlink:href=\"#R\"/>",
    )
}
pub fn runway_element(ring: &str) -> String {
    member(
        "RunwayElement",
        INSTANT,
        &format!("<aixm:associatedRunway xlink:href=\"#R\"/>{}", extent(ring)),
    )
}
pub fn taxiway() -> String {
    member(
        "Taxiway",
        INSTANT,
        "<aixm:associatedAirportHeliport xlink:href=\"#A\"/>",
    )
}
pub fn taxiway_element(ring: &str) -> String {
    member(
        "TaxiwayElement",
        INSTANT,
        &format!(
            "<aixm:associatedTaxiway xlink:href=\"#T\"/>{}",
            extent(ring)
        ),
    )
}
pub fn navaid() -> String {
    member(
        "Navaid",
        INSTANT,
        "<aixm:type>ILS</aixm:type><aixm:runwayDirection xlink:href=\"#D\"/><aixm:servedAirport xlink:href=\"#A\"/>",
    )
}
pub fn unit() -> String {
    member(
        "Unit",
        INSTANT,
        &format!(
            "<aixm:name>TEST ARTCC</aixm:name><aixm:type>ARTCC</aixm:type><aixm:designator>ZQQ</aixm:designator><aixm:position><aixm:ElevatedPoint gml:id=\"P\" {SRS}><gml:pos>30.0 -90.0</gml:pos></aixm:ElevatedPoint></aixm:position>"
        ),
    )
}
pub fn obstacle_area() -> String {
    member(
        "ObstacleArea",
        PERIOD,
        "<aixm:reference_ownerAirport xlink:href=\"#A\"/><aixm:obstacle xlink:href=\"#V\"/>",
    )
}
pub fn vertical_structure() -> String {
    member(
        "VerticalStructure",
        INSTANT,
        "<aixm:name>TOWER 1</aixm:name><aixm:type>TOWER</aixm:type><aixm:lighted>YES</aixm:lighted>",
    )
}
pub fn radar_system() -> String {
    member(
        "RadarSystem",
        INSTANT,
        "<aixm:airportHeliport xlink:href=\"#A\"/>",
    )
}
pub fn primary_radar() -> String {
    member(
        "PrimarySurveillanceRadar",
        INSTANT,
        "<aixm:type>SURVEILLANCE</aixm:type>",
    )
}

/// A message: the event and the given members.
pub fn msg(members: &[&str]) -> String {
    let mut s = String::from(HEAD);
    s.push_str(EVENT);
    for m in members {
        s.push_str(m);
    }
    s.push_str(TAIL);
    s
}

/// A message with every feature kind and a polygon in each of the two element kinds.
pub fn full() -> String {
    msg(&[
        &airport(),
        &runway(),
        &direction(),
        &runway_element(RING),
        &taxiway(),
        &taxiway_element("30.30 -90.30 30.30 -90.40 30.40 -90.40 30.30 -90.30"),
        &navaid(),
        &unit(),
        &obstacle_area(),
        &vertical_structure(),
        &radar_system(),
        &primary_radar(),
    ])
}

/// `s` with the one occurrence of `from` replaced by `to`.
pub fn rep(s: &str, from: &str, to: &str) -> String {
    assert_eq!(s.matches(from).count(), 1, "{from:?} must occur once");
    s.replacen(from, to, 1)
}

pub fn default_stats() -> Stats {
    new_stats(64)
}

pub fn small() -> Notams {
    new_notams(8, 1 << 16, 16, 4096)
}

pub fn decoder() -> Decoder {
    new_decoder(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ))
}

pub fn parser() -> Parser {
    Parser::new(oa_xml::Limits::default()).unwrap()
}

pub fn run_into(xml: &[u8], out: &mut Notams, stats: &mut Stats) -> Result<Outcome, DocError> {
    let mut p = parser();
    let mut d = decoder();
    decode_bytes(&mut p, &mut d, xml, out, stats)
}

/// Decodes `xml`, which must be accepted as one row.
pub fn ok(xml: &str) -> (Notams, Stats) {
    let mut out = small();
    let mut stats = default_stats();
    let o = run_into(xml.as_bytes(), &mut out, &mut stats).unwrap();
    assert_eq!(o.row, 0);
    assert_eq!(out.len, 1);
    check_invariants(&out);
    (out, stats)
}

/// Decodes `xml`, which must be refused whole, leaving nothing behind.
pub fn err(xml: &str) -> DocError {
    let mut out = small();
    let mut stats = default_stats();
    let r = run_into(xml.as_bytes(), &mut out, &mut stats);
    assert_eq!(out.len, 0, "a refused message left rows behind");
    assert_eq!(
        mark(&out),
        mark(&small()),
        "a refused message left data behind"
    );
    r.unwrap_err()
}

pub fn bit(f: Field) -> u64 {
    field_bit(f)
}

/// Every present column is inside its model range and every span inside its table.
pub fn check_invariants(n: &Notams) {
    let all = (1u64 << FIELD_COUNT) - 1;
    assert_eq!(n.present.len(), n.len);
    assert_eq!(n.apt_icao.len(), n.len);
    assert_eq!(n.feature_first.len(), n.len);
    assert_eq!(n.feature_count.len(), n.len);
    assert_eq!(n.poly_vstart.len(), n.poly_vlen.len());
    assert_eq!(n.poly_vstart.len(), n.poly_kind.len());
    assert_eq!(n.poly_vstart.len(), n.poly_feature.len());
    assert_eq!(n.feature_kind.len(), n.feature_time_kind.len());
    assert_eq!(n.feature_kind.len(), n.feature_interpretation.len());
    assert_eq!(n.feature_kind.len(), n.feature_meta.len());
    assert_eq!(n.feature_kind.len(), n.feature_time_meta.len());
    assert_eq!(n.feature_kind.len(), n.feature_errors.len());
    assert_eq!(n.feature_kind.len(), n.feature_begin_ms.len());
    assert_eq!(n.feature_kind.len(), n.feature_end_ms.len());
    assert_eq!(n.feature_kind.len(), n.feature_sequence_number.len());
    assert_eq!(n.feature_kind.len(), n.feature_correction_number.len());
    assert_eq!(n.feature_kind.len(), n.feature_designator.len());
    assert_eq!(n.feature_kind.len(), n.feature_designator_len.len());
    assert_eq!(n.feature_kind.len(), n.feature_icao.len());
    assert_eq!(n.feature_kind.len(), n.feature_icao_len.len());
    assert_eq!(n.vtx_lat.len(), n.vtx_lon.len());
    assert!(n.strings_used <= n.strings.len());
    assert!(n.len <= n.capacity);
    assert!(n.vtx_lat.len() <= n.vtx_capacity);
    assert!(n.poly_vstart.len() <= n.poly_capacity);
    assert!(n.feature_kind.len() <= n.feature_capacity);
    let max_ms = 253_402_300_799_999i64;
    for i in 0..n.len {
        let row = row_at(n, i).unwrap();
        let feature_end = row.feature_first.checked_add(row.feature_count).unwrap();
        assert!(feature_end as usize <= n.feature_kind.len());
        let p = n.present[i];
        let e = n.errors[i];
        assert_eq!(p & e, 0, "row {i}: a field is both present and an error");
        assert_eq!((p | e) & !all, 0, "row {i}: a bit outside the field set");
        let has = |f: Field| p & bit(f) != 0;
        for (f, v) in [
            (Field::Issued, n.issued_ms[i]),
            (Field::Start, n.start_ms[i]),
            (Field::End, n.end_ms[i]),
            (Field::Canceled, n.canceled_ms[i]),
        ] {
            if has(f) {
                assert!(
                    (0..=max_ms).contains(&v),
                    "row {i}: {} = {v}",
                    field_name(f)
                );
            }
        }
        if has(Field::Centre) {
            assert!((-900_000_000..=900_000_000).contains(&n.lat_e7[i]));
            assert!((-1_800_000_000..=1_800_000_000).contains(&n.lon_e7[i]));
        }
        if has(Field::Radius) {
            assert!((1852..=999 * 1852).contains(&n.radius_m[i]));
        }
        for (f, ft, r) in [
            (Field::Floor, n.floor_ft[i], n.floor_ref[i]),
            (Field::Ceiling, n.ceil_ft[i], n.ceil_ref[i]),
        ] {
            if has(f) {
                assert!(ft == ALT_UNLIMITED || (-2000..=100_000).contains(&ft));
                assert!((REF_MSL..=REF_FL).contains(&r));
            }
        }
        if has(Field::Type) {
            assert!((TYPE_NEW..=TYPE_CANCEL).contains(&n.ntype[i]));
        }
        if has(Field::Classification) {
            assert!((CLASS_INTL..=CLASS_MIL).contains(&n.class[i]));
        }
        assert!(n.traffic[i] <= 7 && n.purpose[i] <= 31 && n.scope[i] <= 15);
        assert_eq!(n.feat_mask[i] & !0x1ffe, 0);
        if n.feat_kind[i] != 0 {
            assert!(n.feat_mask[i] & (1 << n.feat_kind[i]) != 0);
        } else {
            assert_eq!(n.feat_mask[i], 0);
        }
        // text and schedule
        for (f, off, len) in [
            (Field::Text, n.text_off[i], n.text_len[i]),
            (Field::Schedule, n.sched_off[i], n.sched_len[i]),
        ] {
            if has(f) {
                assert!(len > 0 && (off + len) as usize <= n.strings_used);
                assert!(
                    std::str::from_utf8(&n.strings[off as usize..(off + len) as usize]).is_ok()
                );
            }
        }
        // polygons
        let polys = polygons_of(n, i);
        assert!(polys.end <= n.poly_vstart.len());
        assert_eq!(
            has(Field::Polygons),
            !polys.is_empty() && e & bit(Field::Polygons) == 0
        );
        for pl in polys {
            let (lat, lon) = polygon_vertices(n, pl);
            assert!(lat.len() >= 3);
            assert_eq!(lat.len(), n.poly_vlen[pl] as usize);
            assert_eq!(lat.len(), lon.len());
            assert!(lat.iter().all(|v| (-900_000_000..=900_000_000).contains(v)));
            assert!(lon
                .iter()
                .all(|v| (-1_800_000_000..=1_800_000_000).contains(v)));
            assert!((FEAT_AIRPORT..=FEAT_PRIMARY_RADAR).contains(&n.poly_kind[pl]));
            assert!((row.feature_first..feature_end).contains(&n.poly_feature[pl]));
        }
    }
}
