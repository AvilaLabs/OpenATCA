//! Extraction, input and capacity tests with hand-made fixtures. Every identifier is made up;
//! only the element structure follows the surveys (docs/SFDPS_SURVEY.md and the others).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::ingest::{ingest_sfdps, ingest_tbfm, ingest_tfms, new_reader};
use oa_bench::input::{
    file_opener, hash_files, list_inputs, load, load_counted_with_limits, msg_limit, DirKind,
    FeedInput,
};
use oa_bench::legacy::{
    hev, new_legacy, tbfm_kind, tfms_kind, Counts, Limits, NONE, NO_ALT, NO_TIME,
};
use oa_replay::FeedKind;
use oa_sha256::{sha256, to_hex};

const GUFI_A: &str = "11111111-2222-4333-8444-555555555555";
const GUFI_B: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

fn sfdps(messages: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">{messages}</ns5:MessageCollection>"#
    )
}

/// A track message: position, altitude, velocity, optional coast flag, address and hint.
fn th(gufi: &str, ts: &str, pos_time: &str, coast: bool, extra: &str) -> String {
    let coast = if coast {
        r#" coastIndicator="COASTING""#
    } else {
        ""
    };
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="{ts}"><enRoute xsi:type="ns5:NasEnRouteType"><position xsi:type="ns5:NasAircraftPositionType" positionTime="{pos_time}"{coast} reportSource="SURVEILLANCE"><actualSpeed><surveillance uom="KNOTS">454.0</surveillance></actualSpeed><altitude uom="FEET">35000.0</altitude><position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>36.500000 -95.250000</pos></location></position><trackVelocity><x uom="KNOTS">79.0</x><y uom="KNOTS">-447.0</y></trackVelocity></position></enRoute><flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="946" siteSpecificPlanId="237" aircraftIdentification="TST123"/><flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="ACTIVE"/><gufi codeSpace="urn:uuid">{gufi}</gufi>{extra}<flightPlan identifier="KR000001Aa"/></flight></message>"#
    )
}

const ADDR_AND_HINT: &str = r#"<aircraftDescription aircraftAddress="A1B2C3" registration="N1"/><supplementalData xsi:type="ns5:NasSupplementalDataType"><additionalFlightInformation><nameValue name="ADSB_02M_52B" value="-A1B2C3"/></additionalFlightInformation></supplementalData>"#;

fn handoff(gufi: &str, ts: &str, event: &str) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="OH" system="SLC" timestamp="{ts}"><enRoute xsi:type="ns5:NasEnRouteType"><boundaryCrossings xsi:type="ns5:NasUnitBoundaryType"><handoff xsi:type="ns5:NasHandoffType" event="{event}"><receivingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZYY" sectorIdentifier="07"/><transferringUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZXX" sectorIdentifier="12"/></handoff></boundaryCrossings></enRoute><flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="946" siteSpecificPlanId="237" aircraftIdentification="TST123"/><gufi codeSpace="urn:uuid">{gufi}</gufi></flight></message>"#
    )
}

fn route_msg(gufi: &str, ts: &str) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="HX" system="SLC" timestamp="{ts}"><agreed><route xsi:type="ns5:NasRouteType"><expandedRoute xsi:type="ns5:NasExpandedRouteType"><routePoint estimatedTime="2030-01-02T03:00:00Z"><point xsi:type="ns2:FixPointType" fix="KAAA"/></routePoint><routePoint estimatedTime="2030-01-02T03:10:00Z"><point xsi:type="ns2:FixPointType" fix="MIDFX"/></routePoint><routePoint><point xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>40.5 -100.25</pos></location></point></routePoint></expandedRoute></route></agreed><flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="946" siteSpecificPlanId="237" aircraftIdentification="TST123"/><gufi codeSpace="urn:uuid">{gufi}</gufi></flight></message>"#
    )
}

// REQ: OA-BENCH-004, OA-LTJ-053
#[test]
fn sfdps_extraction_reads_track_address_handoff_and_route() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    let doc = sfdps(&format!(
        "{}{}{}{}",
        th(
            GUFI_A,
            "2030-01-02T03:00:05.250Z",
            "2030-01-02T03:00:04Z",
            false,
            ADDR_AND_HINT
        ),
        th(
            GUFI_A,
            "2030-01-02T03:00:17.000Z",
            "2030-01-02T03:00:16Z",
            true,
            ""
        ),
        handoff(GUFI_A, "2030-01-02T03:05:00.000Z", "INITIATION"),
        route_msg(GUFI_B, "2030-01-02T03:01:00.000Z"),
    ));
    ingest_sfdps(&mut leg, &mut r, doc.as_bytes(), 1_893_553_300_000);
    assert_eq!(leg.th.flight.len(), 2);
    assert_eq!(leg.flights.gufi.len(), 2);
    // Position, altitude, velocity, receive time, coast flag.
    assert_eq!(leg.th.lat_e7[0], 365_000_000);
    assert_eq!(leg.th.lon_e7[0], -952_500_000);
    assert_eq!(leg.th.alt_ft[0], 35_000);
    assert_eq!((leg.th.vx_dkt[0], leg.th.vy_dkt[0]), (790, -4470));
    assert_eq!(leg.th.recv_ms[0], 1_893_553_300_000);
    assert_eq!(leg.th.pos_ms[1] - leg.th.pos_ms[0], 12_000);
    assert_eq!((leg.th.coast[0], leg.th.coast[1]), (0, 1));
    assert_eq!(leg.counts.th_coast, 1);
    // Identity: callsign, plan id, address and hint.
    assert_eq!(&leg.flights.callsign[0][..6], b"TST123");
    assert_eq!(&leg.flights.plan_id[0][..10], b"KR000001Aa");
    assert_eq!(leg.flights.addr[0], 0xA1B2C3);
    assert_eq!(leg.flights.addr_hint[0], 0xA1B2C3);
    assert_eq!(leg.flights.addr[1], NONE);
    assert_eq!(leg.counts.th_adsb_hint, 1);
    // Handoff with its event kind and both units.
    assert_eq!(leg.handoffs.flight.len(), 1);
    assert_eq!(leg.handoffs.event[0], hev::INITIATION);
    assert_eq!(&leg.handoffs.from[0][..3], b"ZXX");
    assert_eq!(&leg.handoffs.from[0][4..6], b"12");
    assert_eq!(&leg.handoffs.to[0][..3], b"ZYY");
    assert_eq!(&leg.handoffs.to[0][4..6], b"07");
    // Expanded route: two fixes and one point with coordinates.
    assert_eq!(leg.routes.flight.len(), 1);
    assert_eq!(leg.routes.recv_ms, [1_893_553_300_000]);
    assert_eq!(leg.routes.addr_at_receive, [NONE]);
    assert_eq!(leg.routes.len[0], 3);
    assert_eq!(&leg.route_points.fix[1][..5], b"MIDFX");
    assert_ne!(leg.route_points.est_ms[1], NO_TIME);
    assert_eq!(leg.route_points.lat_e7[2], 405_000_000);
    assert_eq!(leg.route_points.lon_e7[2], -1_002_500_000);
}

#[test]
fn track_without_altitude_is_kept_with_no_alt() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    let msg = th(
        GUFI_A,
        "2030-01-02T03:00:05.250Z",
        "2030-01-02T03:00:04Z",
        false,
        "",
    )
    .replace(r#"<altitude uom="FEET">35000.0</altitude>"#, "");
    ingest_sfdps(&mut leg, &mut r, sfdps(&msg).as_bytes(), NO_TIME);
    assert_eq!(leg.th.alt_ft[0], NO_ALT);
    assert_eq!(leg.counts.th_no_alt, 1);
    assert_eq!(leg.th.recv_ms[0], NO_TIME);
}

const TBFM_DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><env xmlns="urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0" targetNamespace="urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0" envTime="2030-01-02T03:00:00.100Z" envSrce="TMA.ZXX.FAA.GOV"><tma msgTime="2030-01-02T03:00:00.100Z" msgId="1"><air gufi="KR000001Aa" tmaId="G00001" apt="KTBB" dap="KTAA" aid="TST123" airType="AMD"><eta><mfx>MFXAA</mfx><eta_mfx>2030-01-02T03:30:00Z</eta_mfx><eta_dfx>2030-01-02T03:30:00Z</eta_dfx><eta_sfx>2030-01-02T03:30:00Z</eta_sfx><eta_rwy>2030-01-02T03:50:00Z</eta_rwy></eta><sta><mfx>MFXAA</mfx><sta_sfx>2030-01-02T03:31:00Z</sta_sfx><sta_rwy>2030-01-02T03:51:00Z</sta_rwy></sta></air></tma></env>"#;

const TFMS_FLIGHT_ATTRS: &str = r#"acid="TST123" airline="TST" arrArpt="KTBB" cdmPart="false" depArpt="KTAA" fdTrigger="HCS_TRACK_MSG" flightRef="100000001" major="TST" sensitivity="A" sourceFacility="KZXX" sourceTimeStamp="2030-01-02T03:04:05Z""#;
const TFMS_QAID: &str = r#"<nxcm:qualifiedAircraftId aircraftCategory="JET" userCategory="COMMERCIAL"><nxce:aircraftId>TST123</nxce:aircraftId><nxce:computerId><nxce:facilityIdentifier>KZXX</nxce:facilityIdentifier><nxce:idNumber>123</nxce:idNumber></nxce:computerId><nxce:gufi>KR000001Aa</nxce:gufi><nxce:departurePoint><nxce:airport>KTAA</nxce:airport></nxce:departurePoint><nxce:arrivalPoint><nxce:airport>KTBB</nxce:airport></nxce:arrivalPoint></nxcm:qualifiedAircraftId>"#;

fn tfms_doc() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:tfmDataService xmlns:ds="urn:us:gov:dot:faa:atm:tfm:tfmdataservice" xmlns="urn:us:gov:dot:faa:atm:tfm:tfmdataservice" xmlns:fdm="urn:us:gov:dot:faa:atm:tfm:flightdata" xmlns:nxce="urn:us:gov:dot:faa:atm:tfm:tfmdatacoreelements" xmlns:nxcm="urn:us:gov:dot:faa:atm:tfm:flightdatacommonmessages"><fltdOutput><fdm:fltdMessage {TFMS_FLIGHT_ATTRS} msgType="flightPlanInformation"><fdm:flightPlanInformation>{TFMS_QAID}<nxcm:ncsmRouteData><nxcm:eta etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="ESTIMATED" timeValue="2030-01-02T02:05:00Z"/><nxcm:routeOfFlight>KTAA./.ABC..DEF.STAR1.KTBB/0100</nxcm:routeOfFlight></nxcm:ncsmRouteData></fdm:flightPlanInformation></fdm:fltdMessage></fltdOutput></ds:tfmDataService>"#
    )
}

// REQ: OA-BENCH-005
#[test]
fn tbfm_and_tfms_extraction_keep_estimates_with_issue_time() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_tbfm(&mut leg, &mut r, TBFM_DOC.as_bytes());
    assert_eq!(leg.counts.doc_errors[1], 0);
    // eta_mfx, eta_rwy, sta_sfx, sta_rwy.
    assert_eq!(leg.tbfm.kind.len(), 4);
    let kinds: Vec<u8> = leg.tbfm.kind.clone();
    for k in [
        tbfm_kind::ETA_MFX,
        tbfm_kind::ETA_RWY,
        tbfm_kind::STA_SFX,
        tbfm_kind::STA_RWY,
    ] {
        assert!(kinds.contains(&k));
    }
    assert_eq!(&leg.tbfm.plan_id[0][..10], b"KR000001Aa");
    assert_eq!(&leg.tbfm.fix[0][..5], b"MFXAA");
    let issue = leg.tbfm.issue_ms[0];
    assert_ne!(issue, NO_TIME);
    // 2030-01-02T03:00:00.100Z; the ETA at the fix is 30 minutes later (less the 0.1 s).
    let i = kinds.iter().position(|&k| k == tbfm_kind::ETA_MFX).unwrap();
    assert_eq!(
        leg.tbfm.time_ms[i] - leg.tbfm.issue_ms[i],
        30 * 60_000 - 100
    );

    ingest_tfms(&mut leg, &mut r, tfms_doc().as_bytes());
    assert_eq!(leg.counts.doc_errors[2], 0, "{:?}", leg.counts);
    assert_eq!(leg.tfms.kind.len(), 2);
    assert!(leg.tfms.kind.contains(&tfms_kind::ETA));
    assert!(leg.tfms.kind.contains(&tfms_kind::ETD));
    assert_eq!(&leg.tfms.plan_id[0][..10], b"KR000001Aa");
    assert_eq!(&leg.tfms.arr[0][..4], b"KTBB");
    assert_eq!(&leg.tfms.dep[0][..4], b"KTAA");
    assert_ne!(leg.tfms.issue_ms[0], NO_TIME);
}

// REQ: OA-BENCH-006
#[test]
fn rows_past_a_capacity_are_dropped_and_counted() {
    let mut leg = new_legacy();
    leg.limits = Limits {
        flights: 1,
        th: 1,
        handoffs: 0,
        routes: 0,
        route_points: 2,
        beacons: 1,
        assigned_altitude_updates: 2,
        tbfm: 2,
        tfms: 1,
    };
    let mut r = new_reader().unwrap();
    let doc = sfdps(&format!(
        "{}{}{}{}{}",
        th(
            GUFI_A,
            "2030-01-02T03:00:05.250Z",
            "2030-01-02T03:00:04Z",
            false,
            ""
        ),
        th(
            GUFI_A,
            "2030-01-02T03:00:17.000Z",
            "2030-01-02T03:00:16Z",
            false,
            ""
        ),
        th(
            GUFI_B,
            "2030-01-02T03:00:17.000Z",
            "2030-01-02T03:00:16Z",
            false,
            ""
        ),
        handoff(GUFI_A, "2030-01-02T03:05:00.000Z", "INITIATION"),
        route_msg(GUFI_A, "2030-01-02T03:01:00.000Z"),
    ));
    ingest_sfdps(&mut leg, &mut r, doc.as_bytes(), NO_TIME);
    assert_eq!(leg.th.flight.len(), 1);
    assert_eq!(leg.counts.overflow_th, 1);
    assert!(leg.counts.overflow_flights >= 1);
    assert_eq!(leg.counts.overflow_handoffs, 1);
    assert_eq!(leg.counts.overflow_routes, 1);
    ingest_tbfm(&mut leg, &mut r, TBFM_DOC.as_bytes());
    assert_eq!(leg.tbfm.kind.len(), 2);
    assert_eq!(leg.counts.overflow_tbfm, 2);
    ingest_tfms(&mut leg, &mut r, tfms_doc().as_bytes());
    assert_eq!(leg.tfms.kind.len(), 1);
    assert_eq!(leg.counts.overflow_tfms, 1);
}

#[test]
fn documents_that_do_not_decode_are_counted_not_ingested() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_sfdps(&mut leg, &mut r, b"not xml", NO_TIME);
    ingest_sfdps(
        &mut leg,
        &mut r,
        br#"<FDPSMsg><properties/></FDPSMsg>"#,
        NO_TIME,
    );
    ingest_tbfm(&mut leg, &mut r, b"<x/>");
    ingest_tfms(&mut leg, &mut r, b"<x/>");
    assert_eq!(leg.counts.doc_errors, [2, 1, 1]);
    assert_eq!(leg.counts.docs, [2, 1, 1]);
    assert!(leg.th.flight.is_empty() && leg.tbfm.kind.is_empty() && leg.tfms.kind.is_empty());
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("oa-bench-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// REQ: OA-BENCH-022, OA-LTJ-055
#[test]
fn directories_of_msg_files_and_oaf_recordings_load_and_hash() {
    // Raw .msg directory: read in name order, no receive time, no replay hash.
    let dir = scratch("msg");
    let a = sfdps(&th(
        GUFI_A,
        "2030-01-02T03:00:05.250Z",
        "2030-01-02T03:00:04Z",
        false,
        &format!(
            r#"{ADDR_AND_HINT}<assignedAltitude><simple uom="FEET">12000</simple></assignedAltitude>"#
        ),
    ));
    let b = sfdps(&th(
        GUFI_A,
        "2030-01-02T03:00:17.000Z",
        "2030-01-02T03:00:16Z",
        false,
        "",
    ));
    std::fs::write(dir.join("000001.msg"), &b).unwrap();
    std::fs::write(dir.join("000000.msg"), &a).unwrap();
    let (kind, files) = list_inputs(&dir).unwrap();
    assert_eq!(kind, DirKind::Msg);
    assert!(files[0].ends_with("000000.msg"));
    let l = load(&[FeedInput {
        feed: FeedKind::Sfdps,
        dir: dir.clone(),
    }])
    .unwrap();
    assert_eq!(l.legacy.th.flight.len(), 2);
    assert_eq!(l.legacy.th.recv_ms[0], NO_TIME);
    assert!(l.replay_hash.starts_with("not available"));
    // The manifest hash is the hash of the sorted `name:sha256` lines.
    let ha = to_hex(&sha256(a.as_bytes()));
    let hb = to_hex(&sha256(b.as_bytes()));
    let manifest = format!("000000.msg:{ha}\n000001.msg:{hb}\n");
    assert_eq!(l.inputs[0].sha256, to_hex(&sha256(manifest.as_bytes())));
    assert_eq!(l.inputs[0].files, 2);
    assert_eq!(hash_files("x", &files[..1]).unwrap().sha256, ha);

    // .oaf recording: frames carry receive times, and the replay state hash is reported.
    let odir = scratch("oaf");
    let mut file = Vec::new();
    for (seq, (payload, recv_ns)) in [
        (&a, 1_893_553_265_000_000_000i64),
        (&b, 1_893_553_277_000_000_000),
    ]
    .iter()
    .enumerate()
    {
        let mut buf = vec![0u8; payload.len() + 64];
        let n = oa_feed::encode_frame(&mut buf, 1, 0, seq as u64, *recv_ns, 0, payload.as_bytes())
            .unwrap();
        file.extend_from_slice(&buf[..n]);
    }
    std::fs::write(odir.join("sfdps-0001.oaf"), &file).unwrap();
    let l = load(&[FeedInput {
        feed: FeedKind::Sfdps,
        dir: odir.clone(),
    }])
    .unwrap();
    assert_eq!(l.legacy.th.flight.len(), 2);
    assert_eq!(l.legacy.th.recv_ms[0], 1_893_553_265_000);
    assert_eq!(l.replay_hash.len(), 64);
    // Staleness: receive time minus position time.
    assert_eq!(
        l.legacy.th.recv_ms[0] - l.legacy.th.pos_ms[0],
        1_893_553_265_000 - (1_893_553_200_000 + 4_000)
    );
    // The same input twice gives the same replay hash.
    let l2 = load(&[FeedInput {
        feed: FeedKind::Sfdps,
        dir: odir.clone(),
    }])
    .unwrap();
    assert_eq!(l.replay_hash, l2.replay_hash);
    assert!(l.legacy.assigned_altitudes.flight.is_empty());
    let mut sealed = 0;
    let configured = load_counted_with_limits(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: odir.clone(),
        }],
        &file_opener(),
        msg_limit(),
        &mut sealed,
        Limits {
            assigned_altitude_updates: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(sealed, 0);
    assert_eq!(configured.legacy.assigned_altitudes.value_ft, [12_000]);
    assert_eq!(configured.replay_hash, l.replay_hash);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(odir);
}

// REQ: OA-BENCH-024
#[test]
fn real_sample_extraction_counts() {
    let Some(data) = std::env::var_os("OPENATCA_DATA") else {
        eprintln!("OPENATCA_DATA is not set: skipping the real-data extraction test");
        return;
    };
    let scds = std::path::Path::new(&data).join("scds");
    let dirs = [
        (FeedKind::Sfdps, "sfdps-20261114T135147Z"),
        (FeedKind::Tbfm, "tbfm-20261114T141148Z"),
        (FeedKind::Tfms, "tfms-20261114T140943Z"),
    ];
    let mut feeds = Vec::new();
    for (feed, name) in dirs {
        let dir = scds.join(name);
        if !dir.is_dir() {
            eprintln!(
                "{} is absent: skipping the real-data extraction test",
                dir.display()
            );
            return;
        }
        feeds.push(FeedInput { feed, dir });
    }
    let l = load(&feeds).unwrap();
    let c = &l.legacy.counts;
    eprintln!(
        "real sample: {:?}",
        oa_bench::report::extraction_counts(&l.legacy)
    );
    assert_eq!(l.legacy.th.flight.len(), 13_268);
    assert_eq!(l.legacy.handoffs.flight.len(), 2_807);
    assert_eq!(l.legacy.routes.flight.len(), 265);
    assert_eq!(l.legacy.route_points.fix.len(), 1_397);
    assert_eq!(c.doc_errors[1] + c.doc_errors[2], 0);
    assert!(l.legacy.tbfm.kind.len() > 5_000);
    assert!(l.legacy.tfms.kind.len() > 4_000);
    assert_eq!(
        l.legacy.flights.addr.iter().filter(|&&a| a != NONE).count(),
        165
    );
    assert_eq!(c.overflow_th + c.overflow_flights, 0);
}

fn count_of(leg: &oa_bench::legacy::Legacy, label: &str) -> u64 {
    oa_bench::report::extraction_counts(leg)
        .iter()
        .find(|(k, _)| k.contains(label))
        .unwrap_or_else(|| panic!("no count line containing {label:?}"))
        .1
}

// REQ: OA-BENCH-091
#[test]
fn tbfm_rows_refused_for_a_field_are_counted_by_reason() {
    let bad = TBFM_DOC
        .replace("KR000001Aa", "KR000002Bb")
        .replace("2030-01-02T03:30:00Z</eta_mfx>", "soon</eta_mfx>");
    let both = TBFM_DOC.replace(
        "</tma>",
        &format!(
            "{}</tma>",
            &bad[bad.find("<air").unwrap()..bad.find("</tma>").unwrap()]
        ),
    );
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_tbfm(&mut leg, &mut r, both.as_bytes());
    assert_eq!(leg.counts.doc_errors[1], 0);
    assert_eq!(leg.tbfm.kind.len(), 4, "the valid flight is kept");
    assert_eq!(count_of(&leg, "TBFM flights refused for a field"), 1);
    assert_eq!(count_of(&leg, "TBFM refusals, reason time"), 1);
    assert_eq!(
        count_of(
            &leg,
            &format!("TBFM refusals, field #{}", oa_tbfm::Field::EtaMfx as usize)
        ),
        1
    );
}

// REQ: OA-BENCH-091
#[test]
fn tfms_messages_refused_for_a_field_are_counted_by_reason() {
    let one = tfms_doc();
    let m0 = one.find("<fdm:fltdMessage").unwrap();
    let m1 = one.find("</fltdOutput>").unwrap();
    let msg = &one[m0..m1];
    let bad = msg.replace(
        r#"etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z""#,
        r#"etaType="ESTIMATED" timeValue="soon""#,
    );
    let doc = one.replace(msg, &format!("{msg}{bad}"));
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_tfms(&mut leg, &mut r, doc.as_bytes());
    assert_eq!(leg.counts.doc_errors[2], 0, "{:?}", leg.counts);
    assert_eq!(leg.tfms.kind.len(), 2, "the valid message is kept");
    assert_eq!(count_of(&leg, "TFMS messages refused for a field"), 1);
    assert_eq!(count_of(&leg, "TFMS refusals, reason time"), 1);
}

// REQ: OA-BENCH-091
#[test]
fn sfdps_messages_and_fields_refused_are_counted_by_reason() {
    let good = th(
        GUFI_A,
        "2030-01-02T03:00:05.250Z",
        "2030-01-02T03:00:04Z",
        false,
        "",
    );
    let bad_field = th(
        GUFI_B,
        "2030-01-02T03:00:05.250Z",
        "2030-01-02T03:00:04Z",
        false,
        "",
    )
    .replace(
        r#"<altitude uom="FEET">35000.0</altitude>"#,
        r#"<altitude uom="FEET">high</altitude>"#,
    );
    let no_gufi = good.replace(
        &format!(r#"<gufi codeSpace="urn:uuid">{GUFI_A}</gufi>"#),
        "",
    );
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_sfdps(
        &mut leg,
        &mut r,
        sfdps(&format!("{good}{bad_field}{no_gufi}")).as_bytes(),
        NO_TIME,
    );
    assert_eq!(count_of(&leg, "SFDPS messages rejected by the decoder"), 1);
    assert_eq!(
        count_of(&leg, "SFDPS messages rejected, reason missing GUFI"),
        1
    );
    assert!(count_of(&leg, "SFDPS field errors in kept rows") >= 1);
    assert!(count_of(&leg, "SFDPS field errors, field ") >= 1);
}

fn reason(leg: &oa_bench::legacy::Legacy, label: &str) -> u64 {
    leg.counts.reasons.get(label).copied().unwrap_or(0)
}

// REQ: OA-BENCH-129
#[test]
fn sfdps_rows_refused_at_the_flight_table_are_counted_by_reason() {
    let mut leg = new_legacy();
    leg.limits = Limits {
        flights: 1,
        ..Limits::default()
    };
    let mut r = new_reader().unwrap();
    let doc = sfdps(&format!(
        "{}{}{}",
        th(
            GUFI_A,
            "2030-01-02T03:00:05.250Z",
            "2030-01-02T03:00:04Z",
            false,
            ""
        ),
        th(
            GUFI_B,
            "2030-01-02T03:00:17.000Z",
            "2030-01-02T03:00:16Z",
            false,
            ""
        ),
        th(
            GUFI_B,
            "2030-01-02T03:00:27.000Z",
            "2030-01-02T03:00:26Z",
            false,
            ""
        ),
    ));
    ingest_sfdps(&mut leg, &mut r, doc.as_bytes(), NO_TIME);
    assert_eq!(leg.th.flight.len(), 1);
    assert_eq!(
        reason(&leg, "SFDPS rows dropped, reason flight table full"),
        2,
        "{:?}",
        leg.counts.reasons
    );
}

// REQ: OA-BENCH-129
#[test]
fn tbfm_values_without_a_time_are_counted() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    let doc = TBFM_DOC.replace("<eta_rwy>2030-01-02T03:50:00Z</eta_rwy>", "");
    ingest_tbfm(&mut leg, &mut r, doc.as_bytes());
    assert_eq!(leg.tbfm.kind.len(), 3);
    assert_eq!(
        reason(&leg, "TBFM values dropped, reason time absent"),
        1,
        "{:?}",
        leg.counts.reasons
    );
}

// REQ: OA-BENCH-129
#[test]
fn tfms_times_that_are_not_estimates_are_counted() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    let doc = tfms_doc().replace(r#"etdType="ESTIMATED""#, r#"etdType="SCHEDULED""#);
    ingest_tfms(&mut leg, &mut r, doc.as_bytes());
    assert_eq!(leg.tfms.kind.len(), 1);
    assert_eq!(
        reason(&leg, "TFMS times dropped, reason type is not ESTIMATED"),
        1,
        "{:?}",
        leg.counts
    );
}

/// The sealed window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z, Unix nanoseconds.
const SEAL_FROM: i64 = 1_795_131_900_000_000_000;
const SEAL_TO: i64 = 1_795_393_920_000_000_000;

fn oaf_frames(frames: &[(i64, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, (t, doc)) in frames.iter().enumerate() {
        let mut buf = vec![0u8; oa_feed::MAX_FRAME_LEN];
        let n = oa_feed::encode_frame(
            &mut buf,
            1,
            oa_feed::FLAG_BYTES,
            i as u64,
            *t,
            t / 1_000_000,
            doc.as_bytes(),
        )
        .unwrap();
        out.extend_from_slice(&buf[..n]);
    }
    out
}

// REQ: OA-SEAL-014
#[test]
fn sfdps_records_timed_in_a_sealed_window_are_dropped_and_counted() {
    let before = th(
        GUFI_A,
        "2026-11-19T23:44:59.999Z",
        "2026-11-19T23:44:59Z",
        false,
        "",
    );
    let at_from = th(
        GUFI_A,
        "2026-11-19T23:45:00.000Z",
        "2026-11-19T23:44:59Z",
        false,
        "",
    );
    let pos_in = th(
        GUFI_B,
        "2026-11-19T23:44:59.999Z",
        "2026-11-19T23:45:00Z",
        false,
        "",
    );
    let last_in = th(
        GUFI_B,
        "2026-11-23T00:31:59.999Z",
        "2026-11-19T23:44:59Z",
        false,
        "",
    );
    let at_to = th(
        GUFI_B,
        "2026-11-23T00:32:00.000Z",
        "2026-11-23T00:32:00Z",
        false,
        "",
    );
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    // One document each: a document with a message in the window is dropped whole.
    for m in [before, at_from, pos_in, last_in, at_to] {
        ingest_sfdps(&mut leg, &mut r, sfdps(&m).as_bytes(), 1_793_000_000_000);
    }
    // Kept: the message at from - 1 ms and the one at to; dropped: message time at from, position
    // time at from, message time at to - 1 ms.
    assert_eq!(leg.th.flight.len(), 2);
    assert_eq!(leg.counts.sealed_window_dropped, 3);
}

// REQ: OA-SEAL-014
#[test]
fn tbfm_and_tfms_records_timed_in_a_sealed_window_are_dropped_and_counted() {
    for (msg, kept) in [
        ("2026-11-19T23:44:59.999Z", true),
        ("2026-11-19T23:45:00.000Z", false),
        ("2026-11-23T00:31:59.999Z", false),
        ("2026-11-23T00:32:00.000Z", true),
    ] {
        let mut leg = new_legacy();
        let mut r = new_reader().unwrap();
        let doc = TBFM_DOC.replace(
            r#"msgTime="2030-01-02T03:00:00.100Z""#,
            &format!(r#"msgTime="{msg}""#),
        );
        ingest_tbfm(&mut leg, &mut r, doc.as_bytes());
        assert_eq!(leg.tbfm.kind.len(), if kept { 4 } else { 0 }, "{msg}");
        // One message, dropped whole.
        assert_eq!(leg.counts.sealed_window_dropped, u64::from(!kept), "{msg}");
    }
    for (src, kept) in [
        ("2026-11-19T23:44:59Z", true),
        ("2026-11-19T23:45:00Z", false),
        ("2026-11-23T00:31:59Z", false),
        ("2026-11-23T00:32:00Z", true),
    ] {
        let mut leg = new_legacy();
        let mut r = new_reader().unwrap();
        let doc = tfms_doc().replace("2030-01-02T03:04:05Z", src);
        ingest_tfms(&mut leg, &mut r, doc.as_bytes());
        assert_eq!(leg.tfms.kind.len(), if kept { 2 } else { 0 }, "{src}");
        assert_eq!(leg.counts.sealed_window_dropped, u64::from(!kept), "{src}");
    }
}

// REQ: OA-SEAL-014
#[test]
fn load_drops_sealed_frames_by_receive_time_and_a_record_inside_a_frame_received_before_it() {
    let dir = std::env::temp_dir().join(format!("oa-bench-seal-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ok = sfdps(&th(
        GUFI_A,
        "2026-11-19T20:00:00.000Z",
        "2026-11-19T20:00:00Z",
        false,
        "",
    ));
    // Received at from - 1 ns but timed inside the window: the record goes, the frame does not.
    let late = sfdps(&th(
        GUFI_B,
        "2026-11-19T23:45:00.000Z",
        "2026-11-19T23:44:59Z",
        false,
        "",
    ));
    let frames = [
        (SEAL_FROM - 1, ok.as_str()),
        (SEAL_FROM - 1, late.as_str()),
        (SEAL_FROM, ok.as_str()),
        (SEAL_TO - 1, ok.as_str()),
        (SEAL_TO, ok.as_str()),
    ];
    std::fs::write(dir.join("a.oaf"), oaf_frames(&frames)).unwrap();
    let l = load(&[FeedInput {
        feed: FeedKind::Sfdps,
        dir: dir.clone(),
    }])
    .unwrap();
    // Two frames by receive time (at from and at to - 1 ns), one record by its own time.
    assert_eq!(l.legacy.counts.sealed_window_dropped, 1);
    assert_eq!(l.sealed_window_dropped, 3);
    // The record's document is dropped whole, so it is not a document here.
    assert_eq!(l.legacy.counts.docs[0], 2);
    assert!(l
        .notes
        .iter()
        .any(|n| n.starts_with("sealed-window rows dropped: 3")));
    std::fs::remove_dir_all(&dir).ok();
}

// REQ: OA-SEAL-014
#[test]
fn a_message_timed_in_the_window_never_reaches_the_hint_handoff_route_or_flight_tables() {
    // Frame received outside the window (2030): the messages carry their own times.
    let recv = 1_893_553_300_000;
    let doc = |ts: &str, pos: &str| {
        sfdps(&format!(
            "{}{}{}",
            th(GUFI_A, ts, pos, false, ADDR_AND_HINT),
            handoff(GUFI_B, ts, "INITIATION"),
            route_msg("cccccccc-dddd-4eee-8fff-000000000001", ts),
        ))
    };
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_sfdps(
        &mut leg,
        &mut r,
        doc("2026-11-21T03:00:05.250Z", "2026-11-21T03:00:04Z").as_bytes(),
        recv,
    );
    // Dropped whole: every count is as for an empty run, with the three messages counted.
    assert_eq!(
        leg.counts,
        Counts {
            sealed_window_dropped: 3,
            ..Counts::default()
        }
    );
    assert_eq!(leg.counts.th_adsb_hint, 0);
    assert_eq!(leg.handoffs.flight.len(), 0);
    assert_eq!(leg.routes.flight.len(), 0);
    assert_eq!(leg.th.flight.len(), 0);
    assert_eq!(leg.flights.gufi.len(), 0);
    assert_eq!(leg.counts.th_coast, 0);
    // The same three messages timed outside it reach every table.
    let mut leg = new_legacy();
    ingest_sfdps(
        &mut leg,
        &mut r,
        doc("2030-01-02T03:00:05.250Z", "2030-01-02T03:00:04Z").as_bytes(),
        recv,
    );
    assert_eq!(leg.counts.sealed_window_dropped, 0);
    assert_eq!(leg.counts.th_adsb_hint, 1);
    assert_eq!(leg.handoffs.flight.len(), 1);
    assert_eq!(leg.routes.flight.len(), 1);
    assert_eq!(leg.flights.gufi.len(), 3);
}

// REQ: OA-SEAL-014
#[test]
fn a_document_cut_off_by_the_row_capacity_is_dropped_whole_for_a_window_message_after_the_cut() {
    // More messages than the event table holds, the last one timed in the window: the prefix
    // decoder would never reach it, and the scan would read it.
    let recv = 1_893_553_300_000;
    let mini = |gufi: &str, ts: &str| {
        format!(
            r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight centre="ZXX" source="TH" system="SLC" timestamp="{ts}"><gufi codeSpace="urn:uuid">{gufi}</gufi></flight></message>"#
        )
    };
    let msgs: String = (0..oa_replay::pipeline::DOC_ROW_CAPACITY + 8)
        .map(|_| mini(GUFI_A, "2030-01-02T03:00:05.250Z"))
        .collect();
    let last = mini(GUFI_B, "2026-11-21T03:00:05.250Z");
    let n = (oa_replay::pipeline::DOC_ROW_CAPACITY + 8 + 1) as u64;
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_sfdps(
        &mut leg,
        &mut r,
        sfdps(&format!("{msgs}{last}")).as_bytes(),
        recv,
    );
    assert_eq!(
        leg.counts,
        Counts {
            sealed_window_dropped: n,
            ..Counts::default()
        }
    );
    assert_eq!((leg.th.flight.len(), leg.handoffs.flight.len()), (0, 0));
    assert_eq!(leg.flights.gufi.len(), 0);
    // Without the window message the same document is cut off, counted, and read as before.
    let mut leg = new_legacy();
    ingest_sfdps(&mut leg, &mut r, sfdps(&msgs).as_bytes(), recv);
    assert_eq!(leg.counts.sealed_window_dropped, 0);
    assert_eq!(leg.counts.docs[0], 1);
    assert_eq!(leg.counts.rows_not_decoded, 8);
}
