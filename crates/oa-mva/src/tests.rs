#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::check::{clearance, coverage, pdf, rings};
use crate::*;

/// A sector for the fixture builder: name, altitude, rings (exterior first), each ring a list of
/// (longitude, latitude) vertices, closed by the builder.
struct Fix {
    name: &'static str,
    alt: i32,
    rings: Vec<Vec<(f64, f64)>>,
}

fn sq(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn pos_list(r: &[(f64, f64)]) -> String {
    let mut v: Vec<String> = r.iter().map(|(x, y)| format!("{x:.5} {y:.5}")).collect();
    if let Some(f) = v.first().cloned() {
        v.push(f);
    }
    v.join(" ")
}

const NIL: &str = r#" xsi:nil="true" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;

/// One airspace feature in the exact shape the FAA writes (prefixes as in the real files).
fn airspace(f: &Fix, adesc: &str) -> String {
    let mut interiors = String::new();
    for h in f.rings.iter().skip(1) {
        interiors.push_str(&format!(
            "<ns1:interior><ns1:LinearRing><ns1:posList>{}</ns1:posList></ns1:LinearRing></ns1:interior>",
            pos_list(h)
        ));
    }
    let ext = f.rings.first().map(|r| pos_list(r)).unwrap_or_default();
    format!(
        r#"<ns8:hasMember ns2:type="simple"><ns3:Airspace ns1:id="1"><ns1:description ns2:type="simple">{adesc}</ns1:description><ns1:boundedBy{NIL}/><ns3:timeSlice><ns3:AirspaceTimeSlice><ns1:validTime ns2:type="simple"><ns1:TimePeriod><ns1:beginPosition>1980-01-01T14:00:00Z</ns1:beginPosition><ns1:endPosition indeterminatePosition="unknown"/></ns1:TimePeriod></ns1:validTime><ns3:interpretation>BASELINE</ns3:interpretation><ns3:sequenceNumber>1</ns3:sequenceNumber><ns3:type>OTHER</ns3:type><ns3:designator{NIL}/><ns3:localType{NIL}/><ns3:name>{name}</ns3:name><ns3:designatorICAO{NIL}/><ns3:controlType{NIL}/><ns3:upperLowerSeparation{NIL}/><ns3:protectedRoute{NIL}/><ns3:geometryComponent><ns3:AirspaceGeometryComponent><ns3:operation{NIL}/><ns3:operationSequence{NIL}/><ns3:theAirspaceVolume><ns3:AirspaceVolume><ns3:minimumLimit uom="FT">{alt}</ns3:minimumLimit><ns3:minimumLimitReference>MSL</ns3:minimumLimitReference><ns3:horizontalProjection><ns3:Surface srsName="urn:ogc:def:crs:OGC:1.3:CRS84" ns1:id="s"><ns1:patches><ns1:PolygonPatch interpolation="planar"><ns1:exterior><ns1:LinearRing><ns1:posList>{ext}</ns1:posList></ns1:LinearRing></ns1:exterior>{interiors}</ns1:PolygonPatch></ns1:patches></ns3:Surface></ns3:horizontalProjection></ns3:AirspaceVolume></ns3:theAirspaceVolume></ns3:AirspaceGeometryComponent></ns3:geometryComponent></ns3:AirspaceTimeSlice></ns3:timeSlice></ns3:Airspace></ns8:hasMember>"#,
        name = f.name,
        alt = f.alt,
    )
}

fn message(desc: &str, adesc: &str, secs: &[Fix]) -> String {
    let members: String = secs.iter().map(|s| airspace(s, adesc)).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><ns8:AIXMBasicMessage ns1:id="m" xmlns:ns2="http://www.w3.org/1999/xlink" xmlns:ns8="http://www.aixm.aero/schema/5.1/message" xmlns:ns1="http://www.opengis.net/gml/3.2" xmlns:ns3="http://www.aixm.aero/schema/5.1"><ns1:description ns2:type="simple">{desc}</ns1:description><ns1:boundedBy{NIL}/><ns3:sequenceNumber>1</ns3:sequenceNumber><ns3:messageMetadata/>{members}</ns8:AIXMBasicMessage>"#
    )
}

const MVA: &str = "MINIMUM VECTORING ALTITUDE (MVA)";

fn load(desc: &str, secs: &[Fix]) -> Result<(Model, Loaded), MvaError> {
    let mut m = new_model();
    let mut p = new_parser().unwrap();
    let l = load_message(&mut m, &mut p, message(desc, MVA, secs).as_bytes())?;
    Ok((m, l))
}

fn load_xml(xml: &str) -> (Model, Result<Loaded, MvaError>) {
    let mut m = new_model();
    let mut p = new_parser().unwrap();
    let r = load_message(&mut m, &mut p, xml.as_bytes());
    (m, r)
}

/// Two side-by-side squares (A 2500, B 3000), the second with a hole.
fn two_squares() -> Model {
    let a = Fix {
        name: "A",
        alt: 2500,
        rings: vec![sq(-88.0, 42.0, -87.0, 43.0)],
    };
    let b = Fix {
        name: "B",
        alt: 3000,
        rings: vec![
            sq(-87.0, 42.0, -86.0, 43.0),
            sq(-86.75, 42.25, -86.25, 42.75),
        ],
    };
    load("XYZ_MVA_FUS3_2024_v1", &[a, b]).unwrap().0
}

// REQ: OA-MVA-001
#[test]
fn loads_a_chart_in_the_faa_shape() {
    let (m, l) = load(
        "RFD_MVA_FUS3_2024",
        &[Fix {
            name: "A",
            alt: 2500,
            rings: vec![sq(-88.5, 42.25, -88.0, 42.75)],
        }],
    )
    .unwrap();
    assert_eq!((l.sectors, l.rings, l.vertices), (1, 1, 5));
    assert_eq!(sector_count(&m), 1);
    assert_eq!(chart_count(&m), 1);
    assert_eq!(m.set, vec![SetKind::Fus3]);
    assert_eq!(m.min_alt_ft, vec![2500]);
    assert_eq!(facility_text(&m, SectorIx(0)), "RFD");
    assert_eq!(sector_name_text(&m, SectorIx(0)), "A");
    // Stored exactly as 1e-7 degrees: -88.5 is -885,000,000 and 42.25 is 422,500,000.
    let (lat, lon) = ring_vertices(&m, 0);
    assert_eq!(lat.first(), Some(&422_500_000));
    assert_eq!(lon.first(), Some(&-885_000_000));
    assert_eq!(lat.first(), lat.last());
    // The set comes from the message name: FUS5, and a TAV with MIA descriptions.
    let m5 = load(
        "C90_MVA_FUS5_2024_v1",
        &[Fix {
            name: "B",
            alt: 2000,
            rings: vec![sq(0.0, 0.0, 1.0, 1.0)],
        }],
    )
    .unwrap()
    .0;
    assert_eq!(m5.set, vec![SetKind::Fus5]);
    let tav = message(
        "ZAU_TAV_2025",
        "MINIMUM IFR ALTITUDE (MIA)",
        &[Fix {
            name: "GALO01",
            alt: 4000,
            rings: vec![sq(0.0, 0.0, 1.0, 1.0)],
        }],
    );
    let (mt, r) = load_xml(&tav);
    r.unwrap();
    assert_eq!(mt.set, vec![SetKind::Tav]);
    assert_eq!(sector_name_text(&mt, SectorIx(0)), "GALO01");
}

// REQ: OA-MVA-002
#[test]
fn holes_are_rings_with_the_hole_bit() {
    let m = two_squares();
    assert_eq!(sector_count(&m), 2);
    assert_eq!(ring_range(&m, SectorIx(0)), 0..1);
    assert_eq!(ring_range(&m, SectorIx(1)), 1..3);
    assert!(!ring_is_hole(&m, 0));
    assert!(!ring_is_hole(&m, 1));
    assert!(ring_is_hole(&m, 2));
    assert_eq!(sector_range_of_chart(&m, 0), 0..2);
}

// REQ: OA-MVA-003
#[test]
fn refuses_what_it_does_not_understand_and_keeps_the_model() {
    let good = message(
        "RFD_MVA_FUS3_2024",
        MVA,
        &[Fix {
            name: "A",
            alt: 2500,
            rings: vec![sq(0.0, 0.0, 1.0, 1.0)],
        }],
    );
    let (mut m, r) = load_xml(&good);
    r.unwrap();
    let before = m.clone();
    let mut p = new_parser().unwrap();
    // An upper limit is not understood.
    let up = good.replace(
        "<ns3:minimumLimitReference>",
        "<ns3:upperLimit uom=\"FT\">9000</ns3:upperLimit><ns3:minimumLimitReference>",
    );
    match load_message(&mut m, &mut p, up.replace("RFD_", "CID_").as_bytes()) {
        Err(MvaError::UnexpectedElement { name, context, .. }) => {
            assert_eq!(name, "upperLimit");
            assert_eq!(context, "AirspaceVolume");
        }
        other => panic!("{other:?}"),
    }
    // A curve segment instead of a posList, an unknown attribute, a second patch.
    let arc = good
        .replace("<ns1:LinearRing>", "<ns1:Arc>")
        .replace("</ns1:LinearRing>", "</ns1:Arc>");
    assert!(matches!(
        load_message(&mut m, &mut p, arc.replace("RFD_", "CID_").as_bytes()),
        Err(MvaError::UnexpectedElement { .. })
    ));
    let attr = good.replace(
        "interpolation=\"planar\"",
        "interpolation=\"planar\" foo=\"1\"",
    );
    assert!(matches!(
        load_message(&mut m, &mut p, attr.replace("RFD_", "CID_").as_bytes()),
        Err(MvaError::UnexpectedAttribute { .. })
    ));
    let two = good.replace(
        "</ns1:patches>",
        "<ns1:PolygonPatch interpolation=\"planar\"/></ns1:patches>",
    );
    assert!(matches!(
        load_message(&mut m, &mut p, two.replace("RFD_", "CID_").as_bytes()),
        Err(MvaError::Cardinality {
            name: "PolygonPatch",
            found: 2,
            ..
        })
    ));
    // Not an AIXM message at all.
    assert!(matches!(
        load_message(&mut m, &mut p, b"<a xmlns:x=\"y\"/>"),
        Err(MvaError::NotAixmMessage { .. })
    ));
    assert!(matches!(
        load_message(&mut m, &mut p, b"<a"),
        Err(MvaError::Xml(_))
    ));
    assert_eq!(
        m, before,
        "a refused message must leave the model as it was"
    );
}

// REQ: OA-MVA-004
#[test]
fn refuses_bad_values() {
    let base = message(
        "RFD_MVA_FUS3_2024",
        MVA,
        &[Fix {
            name: "A",
            alt: 2500,
            rings: vec![sq(0.0, 0.0, 1.0, 1.0)],
        }],
    );
    let bad = |from: &str, to: &str| {
        let (m, r) = load_xml(&base.replace(from, to));
        assert!(sector_count(&m) == 0);
        r.unwrap_err()
    };
    assert!(matches!(
        bad("uom=\"FT\"", "uom=\"M\""),
        MvaError::BadValue {
            field: "minimumLimit unit",
            ..
        }
    ));
    assert!(matches!(
        bad("MSL", "AGL"),
        MvaError::BadValue {
            field: "minimumLimitReference",
            ..
        }
    ));
    assert!(matches!(
        bad("CRS84", "CRS83"),
        MvaError::BadValue {
            field: "Surface srsName",
            ..
        }
    ));
    assert!(matches!(
        bad("planar", "spherical"),
        MvaError::BadValue { .. }
    ));
    assert!(matches!(
        bad("BASELINE", "TEMPDELTA"),
        MvaError::BadValue {
            field: "interpretation",
            ..
        }
    ));
    assert!(matches!(
        bad(">OTHER<", ">SECTOR<"),
        MvaError::BadValue {
            field: "airspace type",
            ..
        }
    ));
    assert!(matches!(
        bad(">2500<", ">18001<"),
        MvaError::BadValue {
            field: "minimumLimit",
            ..
        }
    ));
    assert!(matches!(
        bad(">2500<", ">-5<"),
        MvaError::BadValue {
            field: "minimumLimit",
            ..
        }
    ));
    assert!(matches!(
        bad(">2500<", ">25.5<"),
        MvaError::BadValue {
            field: "minimumLimit",
            ..
        }
    ));
    assert!(matches!(
        bad("<ns3:name>A<", "<ns3:name>A B<"),
        MvaError::BadValue {
            field: "sector name",
            ..
        }
    ));
    assert!(matches!(
        bad("<ns3:name>A<", "<ns3:name>ABCDEFGHI<"),
        MvaError::BadValue {
            field: "sector name",
            ..
        }
    ));
    assert!(matches!(
        bad("RFD_MVA_FUS3", "RFD_MVA_FUS4"),
        MvaError::UnknownChartName { .. }
    ));
    assert!(matches!(
        bad("RFD_MVA", "rfd_MVA"),
        MvaError::UnknownChartName { .. }
    ));
    assert!(matches!(
        bad(
            "MINIMUM VECTORING ALTITUDE (MVA)",
            "MINIMUM IFR ALTITUDE (MIA)"
        ),
        MvaError::BadValue {
            field: "Airspace description",
            ..
        }
    ));
    // A time period that ends is not open ended.
    assert!(matches!(
        bad(
            "<ns1:endPosition indeterminatePosition=\"unknown\"/>",
            "<ns1:endPosition>2030-01-01T00:00:00Z</ns1:endPosition>"
        ),
        MvaError::BadValue { .. }
    ));
    // Longitude 181, latitude 91, three-dimensional coordinates, odd count, 3 vertices.
    let full = "0.00000 0.00000 1.00000 0.00000 1.00000 1.00000 0.00000 1.00000 0.00000 0.00000";
    assert!(matches!(
        bad(
            full,
            "181.00000 0.00000 1.00000 0.00000 1.00000 1.00000 0.00000 1.00000 181.00000 0.00000"
        ),
        MvaError::BadValue {
            field: "posList longitude",
            ..
        }
    ));
    assert!(matches!(
        bad(
            full,
            "0.00000 91.00000 1.00000 0.00000 1.00000 1.00000 0.00000 1.00000 0.00000 91.00000"
        ),
        MvaError::BadValue {
            field: "posList latitude",
            ..
        }
    ));
    assert!(matches!(
        bad(
            full,
            "0.00000 0.00000 1.00000 0.00000 1.00000 1.00000 0.00000 1.00000 0.00000"
        ),
        MvaError::BadValue { .. }
    ));
    assert!(matches!(
        bad(full, "0.00000 0.00000 1.00000 0.00000 0.00000 0.00000"),
        MvaError::BadValue { .. }
    ));
    assert!(matches!(
        bad("<ns1:posList>", "<ns1:posList srsDimension=\"3\">"),
        MvaError::BadValue {
            field: "posList srsDimension",
            ..
        }
    ));
    // Not closed.
    assert!(matches!(
        bad(
            full,
            "0.00000 0.00000 1.00000 0.00000 1.00000 1.00000 0.00000 1.00000 5.00000 5.00000"
        ),
        MvaError::RingNotClosed { .. }
    ));
}

// REQ: OA-MVA-005
#[test]
fn refuses_duplicates_and_bounds() {
    let a = || Fix {
        name: "A",
        alt: 2500,
        rings: vec![sq(0.0, 0.0, 1.0, 1.0)],
    };
    assert!(matches!(
        load("RFD_MVA_FUS3_2024", &[a(), a()]).unwrap_err(),
        MvaError::DuplicateSector { .. }
    ));
    let mut m = new_model();
    let mut p = new_parser().unwrap();
    let x = message("RFD_MVA_FUS3_2024", MVA, &[a()]);
    load_message(&mut m, &mut p, x.as_bytes()).unwrap();
    assert!(matches!(
        load_message(
            &mut m,
            &mut p,
            x.replace("FUS3_2024", "FUS3_2025").as_bytes()
        ),
        Err(MvaError::DuplicateChart { .. })
    ));
    // A different set of the same facility is fine.
    load_message(&mut m, &mut p, x.replace("FUS3", "FUS5").as_bytes()).unwrap();
    assert_eq!(chart_count(&m), 2);
    // A sector wider than the span bound; a file over the size bound.
    let wide = Fix {
        name: "W",
        alt: 2500,
        rings: vec![sq(0.0, 0.0, 21.0, 1.0)],
    };
    assert!(matches!(
        load("WID_MVA_FUS3_2024", &[wide]).unwrap_err(),
        MvaError::Limit {
            what: "sector extent in degrees",
            ..
        }
    ));
    let big = vec![b' '; MAX_FILE_BYTES as usize + 1];
    assert_eq!(
        load_message(&mut m, &mut p, &big),
        Err(MvaError::FileTooLarge(MAX_FILE_BYTES + 1))
    );
    // Too many vertices in one ring.
    let n = MAX_VERTICES_PER_RING + 10;
    let ring: Vec<(f64, f64)> = (0..n)
        .map(|i| (i as f64 * 1e-5, if i % 2 == 0 { 0.0 } else { 1e-5 }))
        .collect();
    let big_ring = Fix {
        name: "R",
        alt: 2500,
        rings: vec![ring],
    };
    assert!(matches!(
        load("BIG_MVA_FUS3_2024", &[big_ring]).unwrap_err(),
        MvaError::Limit {
            what: "vertices per ring",
            ..
        }
    ));
    // The documented bounds themselves.
    assert_eq!(MAX_ALT_FT, 18_000);
    const { assert!(MAX_SECTORS_PER_CHART >= 37 * 10) };
}

// REQ: OA-MVA-006
#[test]
fn degrees_parse_exactly_without_floats() {
    use crate::aixm::parse_deg_e7;
    assert_eq!(parse_deg_e7(b"-88.82035", 180), Some(-888_203_500));
    assert_eq!(parse_deg_e7(b"42.44059", 90), Some(424_405_900));
    assert_eq!(parse_deg_e7(b"0", 90), Some(0));
    assert_eq!(parse_deg_e7(b"-0.0000001", 90), Some(-1));
    assert_eq!(parse_deg_e7(b"90", 90), Some(900_000_000));
    assert_eq!(parse_deg_e7(b"90.0000001", 90), None);
    assert_eq!(parse_deg_e7(b"180", 180), Some(1_800_000_000));
    assert_eq!(parse_deg_e7(b"0.12345678", 90), None);
    for bad in [
        &b""[..],
        b"-",
        b".5",
        b"5.",
        b"+5",
        b"1e3",
        b"1,5",
        b"1234",
        b"--1",
        b"NaN",
        b"1.2.3",
    ] {
        assert_eq!(
            parse_deg_e7(bad, 180),
            None,
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

// REQ: OA-MVA-007
#[test]
fn min_altitude_is_the_highest_containing_sector() {
    let m = two_squares();
    let q = |lat, lon| min_altitude_at(&m, lat, lon, SetKind::Fus3).unwrap();
    assert_eq!(
        q(42.5, -87.5),
        Some(MinAlt {
            min_alt_ft: 2500,
            sectors: 1
        })
    );
    assert_eq!(
        q(42.1, -86.9),
        Some(MinAlt {
            min_alt_ft: 3000,
            sectors: 1
        })
    );
    // Outside every sector: no altitude is invented.
    assert_eq!(q(44.0, -87.5), None);
    // Another set has nothing here.
    assert_eq!(
        min_altitude_at(&m, 42.5, -87.5, SetKind::Tav).unwrap(),
        None
    );
    // On the shared edge both sectors contain the point and the higher value wins.
    assert_eq!(
        q(42.5, -87.0),
        Some(MinAlt {
            min_alt_ft: 3000,
            sectors: 2
        })
    );
    // The shared corner.
    assert_eq!(
        q(42.0, -87.0),
        Some(MinAlt {
            min_alt_ft: 3000,
            sectors: 2
        })
    );
    let mut hit = Vec::new();
    sectors_containing(&m, 42.5, -87.0, SetKind::Fus3, &mut hit).unwrap();
    assert_eq!(hit, vec![SectorIx(0), SectorIx(1)]);
}

// REQ: OA-MVA-008
#[test]
fn edges_and_vertices_are_inside_and_holes_are_outside() {
    let m = two_squares();
    let q = |lat, lon| {
        min_altitude_at(&m, lat, lon, SetKind::Fus3)
            .unwrap()
            .map(|a| a.min_alt_ft)
    };
    // Outer edges and a vertex of sector B.
    assert_eq!(q(42.0, -86.5), Some(3000));
    assert_eq!(q(43.0, -86.0), Some(3000));
    assert_eq!(q(42.5, -86.0), Some(3000));
    // Strictly inside the hole of B: not in B, and A does not reach it.
    assert_eq!(q(42.5, -86.5), None);
    // On the hole's edge and at its corner: inside B (conservative).
    assert_eq!(q(42.25, -86.5), Some(3000));
    assert_eq!(q(42.5, -86.75), Some(3000));
    assert_eq!(q(42.25, -86.75), Some(3000));
    // One unit of the stored resolution (1e-7 degree) off an edge is outside.
    assert_eq!(q(43.000_000_1, -86.5), None);
    assert_eq!(q(42.5, -85.999_999_9), None);
    // A point just inside a hole edge is outside; just outside is inside.
    assert_eq!(q(42.5, -86.749_999_9), None);
    assert_eq!(q(42.5, -86.750_000_1), Some(3000));
    // A diagonal edge is exact too: the triangle (0,0)-(2,0)-(0,2), points on and off the
    // hypotenuse.
    let t = Fix {
        name: "T",
        alt: 2200,
        rings: vec![vec![(10.0, 10.0), (12.0, 10.0), (10.0, 12.0)]],
    };
    let (tm, _) = load("TRI_MVA_FUS5_2024", &[t]).unwrap();
    let qt = |lat, lon| {
        min_altitude_at(&tm, lat, lon, SetKind::Fus5)
            .unwrap()
            .map(|a| a.min_alt_ft)
    };
    assert_eq!(qt(11.0, 11.0), Some(2200));
    assert_eq!(qt(11.000_000_1, 11.0), None);
    assert_eq!(qt(10.999_999_9, 11.0), Some(2200));
    assert_eq!(
        locate_in_ring(&[0, 10, 10, 0, 0], &[0, 0, 10, 10, 0], 5, 5),
        Where::Inside
    );
    assert_eq!(
        locate_in_ring(&[0, 10, 10, 0, 0], &[0, 0, 10, 10, 0], 5, 10),
        Where::Boundary
    );
    assert_eq!(
        locate_in_ring(&[0, 10, 10, 0, 0], &[0, 0, 10, 10, 0], 5, 11),
        Where::Outside
    );
    // Ray passing exactly through a vertex must not miscount.
    assert_eq!(
        locate_in_ring(&[0, 10, 5, 10, 0, 0], &[0, 0, 5, 10, 10, 0], 5, -3),
        Where::Outside
    );
    assert_eq!(
        locate_in_ring(&[0, 10, 5, 10, 0, 0], &[0, 0, 5, 10, 10, 0], 5, 2),
        Where::Inside
    );
}

// REQ: OA-MVA-009
#[test]
fn radius_lookahead_counts_edges_not_just_vertices() {
    // One long thin sector: a 4 degree wide rectangle with vertices only at its corners. A point
    // 5 nm north of the middle of its north edge is 5 nm from the sector, 120+ nm from every
    // vertex.
    let long = Fix {
        name: "L",
        alt: 3000,
        rings: vec![sq(-90.0, 41.0, -86.0, 42.0)],
    };
    let far = Fix {
        name: "F",
        alt: 2000,
        rings: vec![sq(-83.0, 41.0, -82.0, 42.0)],
    };
    let (m, _) = load("LNG_MVA_FUS3_2024", &[long, far]).unwrap();
    let lat = 42.0 + 5.0 / 60.0;
    let r = |nm| sectors_within_radius(&m, lat, -88.0, nm, SetKind::Fus3).unwrap();
    assert_eq!(r(4.0), vec![]);
    assert_eq!(r(5.5), vec![SectorIx(0)]);
    assert_eq!(r(50.0), vec![SectorIx(0)]);
    // Inside a sector the distance is 0.
    assert_eq!(
        sector_distance_nm(&m, SectorIx(0), 41.5, -88.0).unwrap(),
        0.0
    );
    // The distance never exceeds the true one: 1 degree of latitude is 60.04 nm on WGS-84 and
    // the metric says 59.7.
    let d = sector_distance_nm(&m, SectorIx(0), 43.0, -88.0).unwrap();
    assert!((59.0..=60.1).contains(&d), "{d}");
    // Along a parallel at 42 N one degree of longitude is about 44.7 nm; the metric gives less.
    let d = sector_distance_nm(&m, SectorIx(0), 41.5, -85.0).unwrap();
    assert!(d <= 44.8 && d > 40.0, "{d}");
    assert_eq!(
        sectors_within_radius(&m, 41.5, -85.0, 60.0, SetKind::Fus3).unwrap(),
        vec![SectorIx(0)]
    );
    // The far sector only appears for a radius that reaches it.
    let r2 = sectors_within_radius(&m, 41.5, -85.0, 100.0, SetKind::Fus3).unwrap();
    assert_eq!(r2, vec![SectorIx(0), SectorIx(1)]);
    // A hole: a point in the hole of a sector is within a hole-edge distance of it.
    let ring = Fix {
        name: "H",
        alt: 2600,
        rings: vec![sq(0.0, 0.0, 4.0, 4.0), sq(1.0, 1.0, 3.0, 3.0)],
    };
    let (hm, _) = load("HOL_MVA_FUS3_2024", &[ring]).unwrap();
    assert!(min_altitude_at(&hm, 2.0, 2.0, SetKind::Fus3)
        .unwrap()
        .is_none());
    assert_eq!(
        sectors_within_radius(&hm, 2.0, 2.0, 30.0, SetKind::Fus3).unwrap(),
        vec![]
    );
    assert_eq!(
        sectors_within_radius(&hm, 2.0, 2.0, 61.0, SetKind::Fus3).unwrap(),
        vec![SectorIx(0)]
    );
    // Another set is never listed.
    assert!(sectors_within_radius(&hm, 2.0, 2.0, 61.0, SetKind::Tav)
        .unwrap()
        .is_empty());
}

// REQ: OA-MVA-010
#[test]
fn queries_refuse_bad_input() {
    let m = two_squares();
    for (la, lo) in [
        (91.0, 0.0),
        (0.0, 181.0),
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
        (-90.1, 0.0),
    ] {
        assert_eq!(
            min_altitude_at(&m, la, lo, SetKind::Fus3),
            Err(MvaError::BadCoordinate)
        );
        assert_eq!(
            sectors_within_radius(&m, la, lo, 5.0, SetKind::Fus3),
            Err(MvaError::BadCoordinate)
        );
    }
    for r in [0.0, -1.0, f64::NAN, f64::INFINITY, MAX_RADIUS_NM + 0.001] {
        assert_eq!(
            sectors_within_radius(&m, 42.0, -87.0, r, SetKind::Fus3),
            Err(MvaError::BadRadius)
        );
    }
    assert!(sectors_within_radius(&m, 42.0, -87.0, MAX_RADIUS_NM, SetKind::Fus3).is_ok());
    // The poles and the antimeridian are valid coordinates, and an empty model answers None.
    let e = new_model();
    assert_eq!(min_altitude_at(&e, 90.0, 180.0, SetKind::Fus3), Ok(None));
    assert_eq!(min_altitude_at(&e, -90.0, -180.0, SetKind::Fus3), Ok(None));
    assert!(sectors_within_radius(&e, 90.0, 0.0, 10.0, SetKind::Tav)
        .unwrap()
        .is_empty());
    assert!(!sector_contains(&e, SectorIx(7), 0, 0));
    assert_eq!(
        sector_distance_nm(&e, SectorIx::NONE, 0.0, 0.0).unwrap(),
        f64::INFINITY
    );
}

fn raw_model(rings_: &[&[(i32, i32)]]) -> Model {
    // Builds one sector straight into the columns: (lat, lon) in e7, each ring as given.
    let mut m = new_model();
    m.facility.push(*b"TST\0");
    m.set.push(SetKind::Fus3);
    m.sub.push([0; MAX_SUB_BYTES]);
    m.sector_id.push(*b"A\0\0\0\0\0\0\0");
    m.min_alt_ft.push(2500);
    m.ring_first.push(0);
    m.ring_count.push(rings_.len() as u32);
    for (k, r) in rings_.iter().enumerate() {
        m.ring_vert_first.push(m.vert_lat_e7.len() as u32);
        m.ring_vert_count.push(r.len() as u32);
        for &(la, lo) in *r {
            m.vert_lat_e7.push(la);
            m.vert_lon_e7.push(lo);
        }
        if k > 0 {
            m.ring_hole.resize(1, 0);
            m.ring_hole[0] |= 1 << k;
        }
    }
    let ext = rings_[0];
    m.bbox_lat_min.push(ext.iter().map(|p| p.0).min().unwrap());
    m.bbox_lat_max.push(ext.iter().map(|p| p.0).max().unwrap());
    m.bbox_lon_min.push(ext.iter().map(|p| p.1).min().unwrap());
    m.bbox_lon_max.push(ext.iter().map(|p| p.1).max().unwrap());
    m
}

// REQ: OA-MVA-011
#[test]
fn ring_integrity_counts_defects_without_repairing() {
    // A clean square: counter-clockwise, nothing to report.
    let ok = raw_model(&[&[(0, 0), (0, 100), (100, 100), (100, 0), (0, 0)]]);
    let r = rings::model_rings(&ok);
    assert_eq!(
        (r.proper_crossings, r.touches, r.spikes, r.not_closed),
        (0, 0, 0, 0)
    );
    assert_eq!(r.clockwise_exteriors, 0);
    let cw = raw_model(&[&[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]]);
    assert_eq!(rings::model_rings(&cw).clockwise_exteriors, 1);
    // A bow tie crosses itself once.
    let bow = raw_model(&[&[(0, 0), (100, 100), (0, 100), (100, 0), (0, 0)]]);
    let r = rings::model_rings(&bow);
    assert_eq!(r.proper_crossings, 1);
    // A ring that touches itself at a vertex (figure eight pinched at a point).
    let pinch = raw_model(&[&[
        (0, 0),
        (50, 50),
        (0, 100),
        (100, 100),
        (50, 50),
        (100, 0),
        (0, 0),
    ]]);
    let r = rings::model_rings(&pinch);
    assert_eq!(
        (r.proper_crossings, r.touches),
        (0, 4),
        "a pinch vertex joins two edge pairs on each side: four non-adjacent pairs touch"
    );
    // A spike: out and back along one line.
    let spike = raw_model(&[&[
        (0, 0),
        (0, 100),
        (0, 150),
        (0, 100),
        (100, 100),
        (100, 0),
        (0, 0),
    ]]);
    assert!(rings::model_rings(&spike).spikes >= 1);
    // An open ring is counted as not closed (the loader would have refused it).
    let open = raw_model(&[&[(0, 0), (0, 100), (100, 100), (100, 0)]]);
    assert_eq!(rings::model_rings(&open).not_closed, 1);
    // A hole outside its exterior; a repeated vertex; a zero-area ring.
    let ho = raw_model(&[
        &[(0, 0), (0, 100), (100, 100), (100, 0), (0, 0)],
        &[(500, 500), (500, 600), (600, 600), (600, 500), (500, 500)],
    ]);
    assert_eq!(rings::model_rings(&ho).holes_outside, 1);
    let rep = raw_model(&[&[(0, 0), (0, 0), (0, 100), (100, 100), (100, 0), (0, 0)]]);
    assert_eq!(rings::model_rings(&rep).repeated_vertices, 1);
    let flat = raw_model(&[&[(0, 0), (0, 100), (0, 200), (0, 0)]]);
    assert_eq!(rings::model_rings(&flat).zero_area, 1);
    // A hole crossing its exterior's edge counts as a crossing.
    let cr = raw_model(&[
        &[(0, 0), (0, 100), (100, 100), (100, 0), (0, 0)],
        &[(50, 50), (50, 150), (80, 150), (80, 50), (50, 50)],
    ]);
    assert_eq!(rings::model_rings(&cr).proper_crossings, 2);
}

// REQ: OA-MVA-012
#[test]
fn coverage_finds_gaps_and_overlaps() {
    let c = coverage::COVERAGE_CELL_DEG;
    // Four squares tiling a 0.2 x 0.2 degree block, no gap and no overlap.
    let q = |n, x, y| Fix {
        name: n,
        alt: 2500,
        rings: vec![sq(x, y, x + 0.1, y + 0.1)],
    };
    let tiled = [
        q("A", 0.0, 0.0),
        q("B", 0.1, 0.0),
        q("C", 0.0, 0.1),
        q("D", 0.1, 0.1),
    ];
    let (m, _) = load("TIL_MVA_FUS3_2024", &tiled).unwrap();
    let r = coverage::chart_coverage(&m, 0).unwrap();
    assert!(r.gaps.is_empty(), "{:?}", r.gaps);
    assert!(r.overlaps.is_empty(), "{:?}", r.overlaps);
    // 0.2 x 0.2 degrees at 0.05 N: about 12 x 12 nm.
    assert!(
        (r.covered_area_nm2 - 144.0).abs() < 8.0,
        "{}",
        r.covered_area_nm2
    );
    // Remove D: the outside is connected, so no gap. Remove nothing but shrink B to leave a
    // pocket in the middle: an annulus of sectors around a missing centre.
    let ring = |n, hole: bool| Fix {
        name: n,
        alt: 2500,
        rings: if hole {
            vec![sq(0.0, 0.0, 0.3, 0.3), sq(0.1, 0.1, 0.2, 0.2)]
        } else {
            vec![sq(0.1, 0.1, 0.2, 0.2)]
        },
    };
    let (m, _) = load("PKT_MVA_FUS3_2024", &[ring("A", true)]).unwrap();
    let r = coverage::chart_coverage(&m, 0).unwrap();
    assert_eq!(r.gaps.len(), 1);
    let want = 0.1 * 60.0 * 0.1 * 60.0 * 0.5f64.to_radians().cos().max(0.99);
    assert!(
        (r.gaps[0].area_nm2 - want).abs() < 4.0,
        "{} vs {want}",
        r.gaps[0].area_nm2
    );
    assert!(!r.gaps[0].covered_by_other_facility);
    // Fill the pocket with its own sector: no gap, no overlap (the hole is excluded).
    let (m, _) = load("PKT_MVA_FUS3_2024", &[ring("A", true), ring("B", false)]).unwrap();
    let r = coverage::chart_coverage(&m, 0).unwrap();
    assert!(
        r.gaps.is_empty() && r.overlaps.is_empty(),
        "{:?} {:?}",
        r.gaps,
        r.overlaps
    );
    // Two sectors that overlap by half of one.
    let a = Fix {
        name: "A",
        alt: 2500,
        rings: vec![sq(0.0, 0.0, 0.1, 0.1)],
    };
    let b = Fix {
        name: "B",
        alt: 3000,
        rings: vec![sq(0.05, 0.0, 0.15, 0.1)],
    };
    let (m, _) = load("OVL_MVA_FUS3_2024", &[a, b]).unwrap();
    let r = coverage::chart_coverage(&m, 0).unwrap();
    assert_eq!(r.overlaps.len(), 1);
    assert_eq!(
        (r.overlaps[0].a.as_str(), r.overlaps[0].b.as_str()),
        ("A", "B")
    );
    assert_eq!((r.overlaps[0].alt_a, r.overlaps[0].alt_b), (2500, 3000));
    let cells = (0.05 / c) * (0.1 / c);
    assert!(
        (r.overlaps[0].cells as f64 - cells).abs() < 0.05 * cells,
        "{}",
        r.overlaps[0].cells
    );
    assert!(coverage::chart_coverage(&m, 9).is_err());
}

// REQ: OA-MVA-013
#[test]
fn pdf_labels_are_read_and_compared() {
    // Layout as `pdftotext -raw` prints it, including a number and the next id on one line.
    let text = "B\n26\nA\n25\nQHZ01\n40\nG\n35 C\n26\nNOT FOR NAVIGATION\nPlot Generated by\nSDAT MVA PROJECT NAME:\nRFD_MVA_FUS3_2024\n";
    let labels = pdf::pdf_labels_from_text(text);
    assert_eq!(
        labels,
        vec![
            ("B".to_string(), 2600),
            ("A".to_string(), 2500),
            ("QHZ01".to_string(), 4000),
            ("G".to_string(), 3500),
            ("C".to_string(), 2600),
        ]
    );
    let a = Fix {
        name: "A",
        alt: 2500,
        rings: vec![sq(0.0, 0.0, 1.0, 1.0)],
    };
    let b = Fix {
        name: "B",
        alt: 2700,
        rings: vec![sq(1.0, 0.0, 2.0, 1.0)],
    };
    let d = Fix {
        name: "D",
        alt: 3000,
        rings: vec![sq(2.0, 0.0, 3.0, 1.0)],
    };
    let (m, _) = load("RFD_MVA_FUS3_2024", &[a, b, d]).unwrap();
    let rep = pdf::compare_chart(&m, 0, &labels);
    assert_eq!(rep.matched, 1);
    assert_eq!(rep.mismatched, vec![("B".to_string(), 2700, 2600)]);
    assert_eq!(rep.missing_in_pdf, vec!["D".to_string()]);
    assert_eq!(
        rep.extra_in_pdf,
        vec!["QHZ01".to_string(), "G".to_string(), "C".to_string()]
    );
    assert_eq!(rep.chart, "RFD FUS3");
    let dir = std::path::Path::new("/x");
    assert_eq!(
        pdf::pdf_path_for(dir, "C90", "FUS5"),
        dir.join("C90_MVA_FUS5.pdf")
    );
    assert_eq!(
        pdf::pdf_path_for(dir, "ZAU", "TAV"),
        dir.join("ZAU_TAV.pdf")
    );
}

// REQ: OA-MVA-014
#[test]
fn terrain_scan_takes_samples_inside_the_polygon_only() {
    // A 10 x 10 grid, 0.1 degree pitch, north-west sample centre at (lat 10.0, lon 20.0),
    // elevation = row * 100 + col so the maximum sits at the south-east.
    let w = 10u32;
    let h = 10u32;
    let mut elev = Vec::new();
    for r in 0..h {
        for c in 0..w {
            elev.push((r * 100 + c) as f32);
        }
    }
    let g = oa_tiff::Grid {
        width: w,
        height: h,
        lon0: 20.0,
        lat0: 10.0,
        dlon: 0.1,
        dlat: 0.1,
        elev,
        nodata_count: 0,
        point_registered: true,
        crs: 4326,
        vertical_cs: None,
    };
    // A square covering columns 2..=4 and rows 1..=3 (centres at lon 20.2..20.4, lat 9.9..9.7),
    // with the edges half a sample outside.
    let edges = vec![(20.15, 9.65, 20.15, 9.95), (20.45, 9.65, 20.45, 9.95)];
    let mut acc = clearance::Acc {
        max_m: f32::MIN,
        peak: (0.0, 0.0),
        samples: 0,
        nodata: 0,
    };
    clearance::scan_grid(&g, &edges, &mut acc);
    // Two vertical edges alone enclose nothing without a horizontal pair: rows get crossings
    // from the two edges, so the span between them is filled for rows 1..=3.
    assert_eq!(acc.samples, 9);
    assert_eq!(acc.max_m, 304.0);
    assert!(
        (acc.peak.0 - 9.7).abs() < 1e-9 && (acc.peak.1 - 20.4).abs() < 1e-9,
        "{:?}",
        acc.peak
    );
    // NaN is counted as no data, never a value.
    let mut g2 = g.clone();
    g2.elev[3 * 10 + 4] = f32::NAN;
    let mut acc = clearance::Acc {
        max_m: f32::MIN,
        peak: (0.0, 0.0),
        samples: 0,
        nodata: 0,
    };
    clearance::scan_grid(&g2, &edges, &mut acc);
    assert_eq!((acc.samples, acc.nodata), (8, 1));
    assert_eq!(acc.max_m, 303.0);
    assert_eq!(clearance::tile_name(41, -88), "USGS_1_n42w088.tif");
    assert_eq!(clearance::percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.5), 3.0);
    assert!(clearance::percentile(&[], 0.5).is_nan());
    let mk = |t, o, alt| clearance::SectorClearance {
        sector: SectorIx(0),
        facility: String::new(),
        set: "FUS3",
        sector_id: String::new(),
        min_alt_ft: alt,
        terrain_max_ft: t,
        terrain_peak: None,
        terrain_samples: 0,
        terrain_nodata: 0,
        tiles_missing: 0,
        obstacle_max_ft: o,
        obstacle_peak: None,
        obstacles: 0,
    };
    assert_eq!(
        clearance::margin_ft(&mk(Some(1000.0), Some(1500), 2500)),
        Some(1000.0)
    );
    assert_eq!(
        clearance::margin_ft(&mk(Some(2000.0), Some(1500), 2500)),
        Some(500.0)
    );
    assert_eq!(
        clearance::margin_ft(&mk(None, Some(3000), 2500)),
        Some(-500.0)
    );
    assert_eq!(clearance::margin_ft(&mk(None, None, 2500)), None);
}

fn one_sq(name: &'static str, alt: i32, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Fix> {
    vec![Fix {
        name,
        alt,
        rings: vec![sq(x0, y0, x1, y1)],
    }]
}

// REQ: OA-MVA-021
#[test]
fn sub_area_chart_names_load_with_their_sub_area() {
    let (m, l) = load("SAV_LHW_MVA_2022", &one_sq("A", 1500, 0.0, 0.0, 1.0, 1.0)).unwrap();
    assert_eq!(l.sectors, 1);
    assert_eq!(m.set, vec![SetKind::Sub]);
    assert_eq!(facility_text(&m, SectorIx(0)), "SAV");
    assert_eq!(&m.sub[0], b"LHW\0");
    assert_eq!(&m.chart_sub[0], b"LHW\0");
    assert_eq!(chart_set_text(&m, 0), "SUB LHW");
    // The edition label is optional, a year and a version are accepted.
    for d in [
        "SAV_LHW_MVA",
        "SAV_LHW_MVA_2022_v2",
        "SAV_LHW_MVA_v1",
        "FHU_FHU_MVA_2024",
    ] {
        assert_eq!(
            load(d, &one_sq("A", 1500, 0.0, 0.0, 1.0, 1.0))
                .unwrap()
                .0
                .set,
            vec![SetKind::Sub]
        );
    }
    // Older names are unchanged and keep an empty sub-area.
    let f = load("ALO_MVA_FUS3_2024", &one_sq("A", 2000, 0.0, 0.0, 1.0, 1.0))
        .unwrap()
        .0;
    assert_eq!(&f.sub[0], &[0u8; MAX_SUB_BYTES]);
    assert_eq!(chart_set_text(&f, 0), "FUS3");
    // The same facility and sub-area twice is a duplicate; another sub-area is not.
    let mut m = new_model();
    let mut p = new_parser().unwrap();
    let mut add = |d: &str| {
        load_message(
            &mut m,
            &mut p,
            message(d, MVA, &one_sq("A", 1500, 0.0, 0.0, 1.0, 1.0)).as_bytes(),
        )
    };
    assert!(add("SAV_LHW_MVA_2022").is_ok());
    assert!(add("SAV_HXD_MVA_2022").is_ok());
    assert!(add("SAV_MVA_FUS3_2022").is_ok());
    assert!(matches!(
        add("SAV_LHW_MVA_2023"),
        Err(MvaError::DuplicateChart { set, .. }) if set == "SUB LHW"
    ));
    assert_eq!(chart_count(&m), 3);
}

// REQ: OA-MVA-022
#[test]
fn unknown_chart_names_are_refused_with_a_clear_error() {
    for d in [
        "HLN_MVA_MULTI",
        "ZUA_MVA_CENRAP",
        "TWF_BYI_VOR_MVA",
        "ZLC_GTF_MVA_FUS5",
        "D01_DEN_PUB_MVA_FUS3",
        "ZHN_ITO%20ASR8_MVA",
        "SAV_LHW_MVA_20X2",
        "SAV_LHW_MVA_2022_x",
        "SAV_LHW_MVA_2022_v1_extra",
        "SAV_LHWWW_MVA",
        "SAV_lhw_MVA",
        "SAV_MVA_MVA",
        "SAV",
    ] {
        let r = load(d, &one_sq("A", 1500, 0.0, 0.0, 1.0, 1.0));
        match r {
            Err(MvaError::UnknownChartName { name, .. }) => assert_eq!(name, d),
            other => panic!("{d:?}: {other:?}"),
        }
    }
    let e = load("HLN_MVA_MULTI", &one_sq("A", 1500, 0.0, 0.0, 1.0, 1.0)).unwrap_err();
    assert!(e
        .to_string()
        .contains("unknown chart name \"HLN_MVA_MULTI\""));
}

// REQ: OA-MVA-023
#[test]
fn sub_area_charts_are_queried_apart_and_joined_conservatively() {
    let mut m = new_model();
    let mut p = new_parser().unwrap();
    for (d, alt, x1) in [
        ("SAV_MVA_FUS3_2024", 2000, 2.0),
        ("SAV_LHW_MVA_2022", 2500, 1.0),
        ("CHS_LHW_MVA_2022", 4000, 1.0),
    ] {
        load_message(
            &mut m,
            &mut p,
            message(d, MVA, &one_sq("A", alt, 0.0, 0.0, x1, 1.0)).as_bytes(),
        )
        .unwrap();
    }
    let at = |lat, lon, set| min_altitude_at(&m, lat, lon, set).unwrap();
    // FUS3 does not see the sub-area chart; the Sub set sees both facilities' sub charts.
    assert_eq!(at(0.5, 0.5, SetKind::Fus3).unwrap().min_alt_ft, 2000);
    assert_eq!(at(0.5, 0.5, SetKind::Sub).unwrap().sectors, 2);
    // Joined for SAV: the higher of FUS3 (2000) and SAV's own sub chart (2500); CHS's is ignored.
    let j = min_altitude_with_sub(&m, 0.5, 0.5, SetKind::Fus3, "SAV")
        .unwrap()
        .unwrap();
    assert_eq!((j.min_alt_ft, j.sectors), (2500, 2));
    // Outside the sub chart the FUS3 answer stands; a FUS5 query finds nothing.
    let j = min_altitude_with_sub(&m, 0.5, 1.5, SetKind::Fus3, "SAV")
        .unwrap()
        .unwrap();
    assert_eq!((j.min_alt_ft, j.sectors), (2000, 1));
    // A sub chart alone answers when the set has no chart there.
    let j = min_altitude_with_sub(&m, 0.5, 0.5, SetKind::Fus5, "SAV")
        .unwrap()
        .unwrap();
    assert_eq!((j.min_alt_ft, j.sectors), (2500, 1));
    assert_eq!(
        min_altitude_with_sub(&m, 5.0, 5.0, SetKind::Fus3, "SAV"),
        Ok(None)
    );
    assert_eq!(
        min_altitude_with_sub(&m, 95.0, 0.0, SetKind::Fus3, "SAV"),
        Err(MvaError::BadCoordinate)
    );
    assert_eq!(
        pdf::pdf_path_for(std::path::Path::new("d"), "SAV", "SUB LHW"),
        std::path::Path::new("d/SAV_LHW_MVA.pdf")
    );
}

// REQ: OA-MVA-074
#[test]
fn the_antimeridian_aliases_answer_alike() {
    // East of the seam (-180..-179.9, 5000 ft), a neighbour past it (-179.89..-179.8, 6000 ft)
    // and a sector on the west side (179.9..180, 4000 ft).
    let mk = |x0, x1, alt, name| Fix {
        name,
        alt,
        rings: vec![sq(x0, 50.0, x1, 50.1)],
    };
    let m = load(
        "XYZ_MVA_FUS3_2024_v1",
        &[
            mk(-180.0, -179.9, 5000, "A"),
            mk(-179.89, -179.8, 6000, "B"),
            mk(179.9, 180.0, 4000, "C"),
        ],
    )
    .unwrap()
    .0;
    for lon in [-180.0, 180.0] {
        let r = min_altitude_at(&m, 50.05, lon, SetKind::Fus3)
            .unwrap()
            .unwrap();
        // On A's edge and C's edge: the higher wins, two sectors.
        assert_eq!((r.min_alt_ft, r.sectors), (5000, 2), "lon {lon}");
        let mut v = Vec::new();
        sectors_containing(&m, 50.05, lon, SetKind::Fus3, &mut v).unwrap();
        assert_eq!(v, vec![SectorIx(0), SectorIx(2)], "lon {lon}");
        assert_eq!(sector_distance_nm(&m, SectorIx(0), 50.05, lon), Ok(0.0));
        assert_eq!(sector_distance_nm(&m, SectorIx(2), 50.05, lon), Ok(0.0));
        // B lies 0.11 degrees of longitude (about 4 NM at 50 N) from the seam.
        let d = sector_distance_nm(&m, SectorIx(1), 50.05, lon).unwrap();
        assert!(d > 3.0 && d < 5.0, "lon {lon} d {d}");
        let near = sectors_within_radius(&m, 50.05, lon, 6.0, SetKind::Fus3).unwrap();
        assert_eq!(
            near,
            vec![SectorIx(0), SectorIx(1), SectorIx(2)],
            "lon {lon}"
        );
        let far = sectors_within_radius(&m, 50.05, lon, 2.0, SetKind::Fus3).unwrap();
        assert_eq!(far, vec![SectorIx(0), SectorIx(2)], "lon {lon}");
    }
}
