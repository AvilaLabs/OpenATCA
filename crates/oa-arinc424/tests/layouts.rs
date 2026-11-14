//! Field-position tests (OA-424-007): every record type is decoded from at least three real
//! records copied from the 2026-10-29 FAACIFP18 into `tests/fixtures`, and the values that
//! can be checked against an independent source are asserted. "NASR" values are from the
//! 2026-10-29 NASR CSV subscriber files and are quoted in the comments. The same checks run
//! over the whole file in `nasr_cross.rs` when the data is available.

mod common;

use common::{close, distance_bearing, file_of, records};
use oa_arinc424::{
    controlling_agency, is_clean, is_closed, levels_of_service, mora_value_at, procedures_at,
    runway_designator, Airport, Airspace, AirspaceKind, Altitude, AltitudeDescription,
    AltitudeUnit, BoundaryVia, DistanceOrTime, GridMora, Leg, Localizer, Ndb, PathTerminator,
    ProcedureKind, Record, Runway, TurnDirection, VhfNavaid, Waypoint, WaypointKind,
};

fn airports() -> Vec<Airport> {
    records("airports.txt")
        .into_iter()
        .map(|r| match r {
            Record::Airport(a) => a,
            o => panic!("not an airport: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-002, OA-424-007
#[test]
fn airport_records_decode() {
    let a = airports();
    assert_eq!(a.len(), 3);
    let ord = a.iter().find(|x| x.ident == "KORD").expect("KORD");
    assert_eq!(ord.icao_region, "K5");
    assert_eq!(ord.iata.as_deref(), Some("ORD")); // NASR APT_BASE.ARPT_ID
    assert_eq!(ord.name, "CHICAGO O'HARE INTL"); // NASR ARPT_NAME
    assert!(close(ord.position, 41.97694027, -87.90814972)); // NASR LAT/LONG_DECIMAL
    assert_eq!(ord.elevation_ft, Some(680)); // NASR ELEV 680
    assert_eq!(ord.magnetic_variation, Some(-3.0)); // NASR MAG_VARN 3 W
    assert_eq!(ord.longest_runway_ft, Some(13_000)); // 09C/27C... ORD's longest runway is 13,000 ft
    assert!(ord.ifr_capable);
    assert_eq!(ord.longest_runway_surface, Some('H'));
    assert_eq!(ord.transition_altitude, Some(Altitude::Feet(18_000)));
    assert_eq!(ord.transition_level, Some(Altitude::Feet(18_000)));
    assert_eq!(ord.public_military, Some('C'));
    assert_eq!(ord.north_reference, Some('M'));
    assert_eq!(ord.datum, "NAR");
    assert_eq!(ord.meta.customer_area, "USA");
    assert_eq!(ord.meta.file_record_number, "78845");
    assert_eq!(ord.meta.cycle.to_string(), "2112");

    let mdw = a.iter().find(|x| x.ident == "KMDW").expect("KMDW");
    assert!(close(mdw.position, 41.78564313, -87.75272861)); // NASR
    assert_eq!(mdw.elevation_ft, Some(620)); // NASR ELEV 619.8
    assert_eq!(mdw.magnetic_variation, Some(-4.0)); // NASR 4 W
    assert_eq!(mdw.longest_runway_ft, Some(6_500));

    let msp = a.iter().find(|x| x.ident == "KMSP").expect("KMSP");
    assert_eq!(msp.icao_region, "K3");
    assert!(close(msp.position, 44.88197222, -93.22177777)); // NASR
    assert_eq!(msp.elevation_ft, Some(842)); // NASR 841.8
    assert_eq!(msp.magnetic_variation, Some(0.0)); // NASR 0 E
    assert_eq!(msp.name, "MINNEAPOLIS-ST PAUL INTL/WOLD-"); // 30 characters; NASR has the full name
}

fn runways() -> Vec<Runway> {
    records("runways.txt")
        .into_iter()
        .map(|r| match r {
            Record::Runway(x) => x,
            o => panic!("not a runway: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-002, OA-424-007
#[test]
fn runway_records_decode() {
    let r = runways();
    assert_eq!(r.len(), 5);
    let find = |ap: &str, id: &str| {
        r.iter()
            .find(|x| x.airport_ident == ap && x.ident == id)
            .expect("runway")
    };
    let r04l = find("KORD", "RW04L");
    assert_eq!(runway_designator(r04l), "04L");
    assert_eq!(r04l.airport_region, "K5");
    assert_eq!(r04l.length_ft, 7_500); // NASR APT_RWY RWY_LEN 7500
    assert_eq!(r04l.magnetic_bearing, Some(42.0)); // true 39 + 3 W variation
    assert!(close(r04l.threshold, 41.98165558, -87.91391775)); // NASR APT_RWY_END LAT/LONG_DECIMAL
    assert_eq!(r04l.ellipsoid_height_m, Some(166.2)); // 655.7 ft = 199.9 m, geoid height about -33.7 m
    assert_eq!(r04l.threshold_elevation_ft, Some(656)); // NASR RWY_END_ELEV 655.7
    assert_eq!(r04l.displaced_threshold_ft, 0);
    assert_eq!(r04l.threshold_crossing_height_ft, Some(50));
    assert_eq!(r04l.width_ft, Some(150)); // NASR RWY_WIDTH 150
    assert_eq!(r04l.tch_indicator, Some('D'));
    assert_eq!(r04l.localizer_ident, None);

    let r04r = find("KORD", "RW04R");
    assert_eq!(r04r.length_ft, 8_075); // NASR
    assert!(close(r04r.threshold, 41.95332716, -87.8994185)); // NASR
    assert_eq!(r04r.threshold_elevation_ft, Some(661)); // NASR 661.4
    assert_eq!(r04r.localizer_ident.as_deref(), Some("IFJU"));
    assert_eq!(r04r.localizer_category, Some('1'));

    let r09c = find("KORD", "RW09C");
    assert_eq!(r09c.length_ft, 11_245); // NASR
    assert_eq!(r09c.width_ft, Some(200)); // NASR RWY_WIDTH 200
    assert!(close(r09c.threshold, 41.98830322, -87.93157122)); // NASR
    assert_eq!(r09c.localizer_ident.as_deref(), Some("IOYG"));

    // A displaced threshold: the coordinate is the landing threshold, 758 ft in from the
    // runway end (NASR DISPLACED_THR_LEN 758, LAT/LONG_DISPLACED_THR_DECIMAL).
    let mdw04l = find("KMDW", "RW04L");
    assert_eq!(mdw04l.displaced_threshold_ft, 758);
    assert!(close(mdw04l.threshold, 41.78287669, -87.75928211));
    assert_eq!(mdw04l.threshold_crossing_height_ft, Some(55));
    assert_eq!(mdw04l.tch_indicator, Some('R'));
    let mdw13r = find("KMDW", "RW13R");
    assert_eq!(mdw13r.displaced_threshold_ft, 0);
    assert!(close(mdw13r.threshold, 41.78806969, -87.75880541)); // NASR RWY_END, no displacement
    assert_eq!(mdw13r.width_ft, Some(60));
}

fn localizers() -> Vec<Localizer> {
    records("localizers.txt")
        .into_iter()
        .map(|r| match r {
            Record::Localizer(x) => x,
            o => panic!("not a localizer: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-002, OA-424-007
#[test]
fn localizer_records_decode() {
    let l = localizers();
    assert_eq!(l.len(), 4);
    let abu = l.iter().find(|x| x.ident == "IABU").expect("IABU");
    assert_eq!(abu.airport_ident, "KORD");
    assert_eq!(abu.category, '3'); // NASR ILS_BASE CATEGORY III
    assert_eq!(abu.frequency_mhz, 111.75); // NASR LOC_FREQ
    assert_eq!(abu.runway_ident, "RW27R");
    assert!(close(abu.localizer_position, 42.00283163, -87.93061094)); // NASR ILS_BASE
    assert_eq!(abu.bearing, 273.0); // NASR APCH_BEAR 273.01
    let gs = abu.glide_slope_position.expect("glide slope");
    assert!(close(gs, 42.00394825, -87.90326144)); // NASR ILS_GS
    assert_eq!(abu.glide_slope_angle_deg, Some(3.0)); // NASR G_S_ANGLE 3
    assert_eq!(abu.station_declination, Some(-3.0)); // NASR MAG_VAR 3 W
    assert_eq!(abu.glide_slope_elevation_ft, Some(648)); // NASR SITE_ELEVATION 648.4
    assert_eq!(abu.glide_slope_tch_ft, Some(55));
    assert_eq!(abu.localizer_width_deg, 4.68);

    let fju = l.iter().find(|x| x.ident == "IFJU").expect("IFJU");
    assert_eq!(fju.category, '1');
    assert_eq!(fju.frequency_mhz, 110.10); // NASR 110.1
    assert!(close(fju.localizer_position, 41.97116575, -87.87826752)); // NASR
    assert!(close(
        fju.glide_slope_position.expect("gs"),
        41.954682,
        -87.89565247
    )); // NASR
    assert_eq!(fju.bearing, 44.5); // NASR 44.5

    let oyg = l.iter().find(|x| x.ident == "IOYG").expect("IOYG");
    assert_eq!(oyg.frequency_mhz, 111.90);
    assert_eq!(oyg.bearing, 93.0); // NASR 92.97
    assert!(close(
        oyg.glide_slope_position.expect("gs"),
        41.98941177,
        -87.92747427
    )); // NASR

    // A localizer without glide slope has a blank glide slope position.
    let dlg = l.iter().find(|x| x.ident == "IDLG").expect("IDLG");
    assert_eq!(dlg.glide_slope_position, None);
    assert_eq!(dlg.glide_slope_angle_deg, None);
    assert_eq!(dlg.category, '0');
}

fn vhf() -> Vec<VhfNavaid> {
    records("vhf.txt")
        .into_iter()
        .map(|r| match r {
            Record::VhfNavaid(x) => x,
            o => panic!("not a VHF navaid: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-002, OA-424-007
#[test]
fn vhf_navaid_records_decode() {
    let v = vhf();
    assert_eq!(v.len(), 4);
    let abi = v.iter().find(|x| x.ident == "ABI").expect("ABI");
    assert_eq!(abi.name, "ABILENE"); // NASR NAME
    assert_eq!(abi.icao_region, "K4");
    assert_eq!(abi.frequency_mhz, Some(113.70)); // NASR FREQ 113.7
    assert_eq!(abi.class, ['V', 'T', 'H', 'W', ' ']); // VORTAC, high, weather
    assert!(close(
        abi.vor_position.expect("vor"),
        32.48133058,
        -99.86345369
    )); // NASR
    assert!(close(
        abi.dme_position.expect("dme"),
        32.48133058,
        -99.86345369
    )); // NASR TACAN_DME
    assert_eq!(abi.dme_ident, None); // DME ident blank when it equals the VOR ident
    assert_eq!(abi.station_declination, Some(10.0)); // NASR MAG_VARN 10 E
    assert_eq!(abi.dme_elevation_ft, Some(1809)); // NASR ELEV 1809
    assert_eq!(abi.figure_of_merit, Some('2'));
    assert_eq!(abi.datum, "NAR");
    assert_eq!(abi.airport_ident, None);

    // A DME-only facility has no VOR position and class " DHW ".
    let aat = v.iter().find(|x| x.ident == "AAT").expect("AAT");
    assert_eq!(aat.vor_position, None);
    assert_eq!(aat.class, [' ', 'D', 'H', 'W', ' ']);
    assert_eq!(aat.frequency_mhz, Some(116.65)); // NASR FREQ 116.65
    assert!(close(
        aat.dme_position.expect("dme"),
        41.48335388,
        -120.56154166
    )); // NASR
    assert_eq!(aat.dme_elevation_ft, Some(4378)); // NASR ELEV 4378
    assert_eq!(aat.name, "ALTURAS");

    // VOR and DME at different coordinates (NASR lists only the DME position here).
    let tdd = v.iter().find(|x| x.ident == "TDD").expect("TDD");
    let (vp, dp) = (
        tdd.vor_position.expect("vor"),
        tdd.dme_position.expect("dme"),
    );
    assert!(close(dp, 41.98166416, -100.71903388)); // NASR LAT/LONG_DECIMAL
    assert!(vp.lon < dp.lon - 1e-4, "VOR is west of the DME");

    // An ILS/DME names its airport in cols 7-12.
    let isyk = v.iter().find(|x| x.ident == "ISYK").expect("ISYK");
    assert_eq!(isyk.airport_ident.as_deref(), Some("06FA"));
    assert_eq!(isyk.airport_region.as_deref(), Some("K7"));
    assert_eq!(isyk.class, [' ', 'I', 'T', 'W', ' ']);
    assert_eq!(isyk.vor_position, None);
    assert_eq!(isyk.dme_ident.as_deref(), Some("ISYK"));
}

fn ndbs() -> Vec<Ndb> {
    records("ndb.txt")
        .into_iter()
        .map(|r| match r {
            Record::Ndb(x) => x,
            o => panic!("not an NDB: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-002, OA-424-007
#[test]
fn ndb_records_decode() {
    let n = ndbs();
    assert_eq!(n.len(), 4);
    // Two different NDBs share the identifier AA; NASR NAV_BASE lists both.
    let kenie = n.iter().find(|x| x.name == "KENIE").expect("KENIE");
    assert_eq!(kenie.ident, "AA");
    assert_eq!(kenie.icao_region, "K3");
    assert_eq!(kenie.frequency_khz, Some(365.0)); // NASR FREQ 365
    assert!(close(kenie.position, 47.00905216, -96.8151835)); // NASR
    assert_eq!(kenie.magnetic_variation, Some(4.0)); // NASR MAG_VARN 4 E
    assert_eq!(kenie.class, "HOLW");
    assert_eq!(kenie.datum, "NAR");
    let cedar = n.iter().find(|x| x.name == "CEDAR").expect("CEDAR");
    assert!(close(cedar.position, 33.53328333, -82.61436944)); // NASR
    assert_eq!(cedar.frequency_khz, Some(341.0)); // NASR FREQ 341
    assert_eq!(cedar.magnetic_variation, Some(-4.0)); // NASR 4 W
    let adk = n.iter().find(|x| x.ident == "ADK").expect("ADK");
    assert!(close(adk.position, 51.87187647, -176.6759875)); // NASR NDB/DME
    assert_eq!(adk.frequency_khz, Some(530.0)); // NASR FREQ 530
    assert_eq!(adk.meta.customer_area, "CAN"); // coded under a different customer code
    assert_eq!(adk.icao_region, "PA");
}

fn waypoints(name: &str) -> Vec<Waypoint> {
    records(name)
        .into_iter()
        .map(|r| match r {
            Record::Waypoint(x) => x,
            o => panic!("not a waypoint: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-002, OA-424-007
#[test]
fn enroute_waypoint_records_decode() {
    let w = waypoints("enroute_waypoints.txt");
    assert_eq!(w.len(), 3);
    assert!(w
        .iter()
        .all(|x| x.kind == WaypointKind::Enroute && x.area == "ENRT"));
    let aaarg = &w[0];
    assert_eq!(aaarg.ident, "AAARG");
    assert_eq!(aaarg.icao_region, "K"); // offshore fix: region "K " (FAA readme)
    assert_eq!(aaarg.waypoint_type, "W");
    assert_eq!(aaarg.usage, "B");
    assert!(close(aaarg.position, 32.69396388, -78.05129444)); // NASR FIX_BASE (region K)
    assert_eq!(aaarg.magnetic_variation, Some(-9.4));
    assert_eq!(aaarg.name, "AAARG");
    assert_eq!(aaarg.datum, "NAR");
    let aabbz = &w[1];
    assert_eq!(aabbz.icao_region, "K5");
    assert!(close(aabbz.position, 37.8735, -86.30871111)); // NASR
    assert_eq!(aabbz.usage, "");
    let bitez = &w[2];
    assert_eq!(bitez.icao_region, "K3");
    assert!(close(bitez.position, 44.98201111, -93.75958055)); // NASR
    assert_eq!(bitez.magnetic_variation, Some(0.3));
}

// REQ: OA-424-002, OA-424-007
#[test]
fn terminal_waypoint_records_decode() {
    let w = waypoints("terminal_waypoints.txt");
    assert_eq!(w.len(), 3);
    assert!(w
        .iter()
        .all(|x| x.kind == WaypointKind::Terminal && x.area == "KORD" && x.area_region == "K5"));
    assert_eq!(w[0].ident, "ADAME");
    assert!(close(w[0].position, 41.96913888, -87.65236111)); // NASR FIX_BASE ADAME K5
    assert_eq!(w[0].waypoint_type, "C"); // FAA readme: C = ground and satellite based
    assert_eq!(w[0].magnetic_variation, Some(-4.2));
    assert_eq!(w[1].ident, "ADLMN");
    assert_eq!(w[1].waypoint_type, "W");
    assert_eq!(w[2].ident, "AGZED");
    assert_eq!(w[2].name, "AGZED");
}

// REQ: OA-424-003, OA-424-007
#[test]
fn airway_records_group_into_ordered_fixes() {
    let cifp = file_of(&["airways.txt"]);
    assert!(is_clean(&cifp), "{:?}", cifp.errors);
    assert_eq!(cifp.airways.len(), 3);
    let v9 = &cifp.airways[0];
    assert_eq!(v9.ident, "V9");
    assert_eq!(v9.route_type, 'O'); // conventional
    assert_eq!(v9.level, Some('L'));
    let idents: Vec<_> = v9.fixes.iter().map(|f| f.fix.ident.as_str()).collect();
    assert_eq!(idents, ["LEV", "PEPSI", "GOING", "SAFES", "RAYOP", "WAVEZ"]); // NASR AWY_SEG_ALT FROM_POINT order
    assert!(v9.fixes.windows(2).all(|w| w[0].sequence < w[1].sequence));
    assert_eq!(v9.fixes[0].sequence, 100);
    // LEV is a VOR (section D, blank subsection); PEPSI is an enroute waypoint (EA).
    assert_eq!(
        (v9.fixes[0].fix.section, v9.fixes[0].fix.subsection),
        ('D', ' ')
    );
    assert_eq!(
        (v9.fixes[1].fix.section, v9.fixes[1].fix.subsection),
        ('E', 'A')
    );
    assert_eq!(v9.fixes[1].fix.icao_region, "K4");
    assert_eq!(v9.fixes[0].outbound_course, Some(354.4)); // NASR MAG_COURSE 354.44
    assert_eq!(v9.fixes[0].distance_nm, Some(10.0)); // NASR MAG_COURSE_DIST 10
    assert_eq!(v9.fixes[1].distance_nm, Some(11.0)); // NASR 11
    assert_eq!(v9.fixes[2].distance_nm, Some(17.5)); // NASR 17.52
    assert_eq!(v9.fixes[1].inbound_course, Some(353.4));
    assert_eq!(v9.fixes[0].inbound_course, None);
    assert_eq!(v9.fixes[0].min_altitude, Some(Altitude::Feet(2000))); // NASR MIN_ENROUTE_ALT 2000
    assert_eq!(v9.fixes[3].min_altitude, Some(Altitude::Feet(4000))); // NASR 4000 at SAFES
    assert_eq!(v9.fixes[5].min_altitude, Some(Altitude::Feet(3000))); // NASR 3000 at WAVEZ
    assert_eq!(v9.fixes[0].max_altitude, Some(Altitude::Feet(17_500))); // NASR MAX_AUTH_ALT 17500
    assert_eq!(v9.fixes[0].description, ['V', ' ', ' ', ' ']);

    let q1 = &cifp.airways[1];
    assert_eq!(q1.ident, "Q1");
    assert_eq!(q1.route_type, 'R'); // RNAV (FAA readme)
    assert_eq!(q1.level, Some('H'));
    assert_eq!(q1.fixes.len(), 3);
    assert_eq!(q1.fixes[1].min_altitude, Some(Altitude::Feet(18_000)));
    assert_eq!(q1.fixes[1].max_altitude, Some(Altitude::Feet(45_000)));

    // An airway whose MEA is not known keeps that as a value, not as an error.
    let j1 = &cifp.airways[2];
    assert_eq!(j1.ident, "J1");
    assert_eq!(j1.fixes[0].min_altitude, Some(Altitude::Unknown));
    assert_eq!(j1.fixes[0].fix.icao_region, "MM");
}

fn legs(name: &str) -> Vec<Leg> {
    records(name)
        .into_iter()
        .filter_map(|r| match r {
            Record::Leg(x) => Some(x),
            _ => None,
        })
        .collect()
}

// REQ: OA-424-004, OA-424-007
#[test]
fn sid_legs_decode() {
    let l = legs("sid.txt");
    assert_eq!(l.len(), 6);
    assert!(l.iter().all(|x| x.kind == ProcedureKind::Sid
        && x.airport_ident == "KMSP"
        && x.airport_region == "K3"));
    assert_eq!(l[0].ident, "BITEZ1");
    assert_eq!(l[0].route_type, '5'); // runway transition ("ALL" runways)
    assert_eq!(l[0].transition, "ALL");
    assert_eq!(l[0].sequence, 10);
    let f = l[0].fix.as_ref().expect("fix");
    assert_eq!(
        (
            f.ident.as_str(),
            f.icao_region.as_str(),
            f.section,
            f.subsection
        ),
        ("HHOBA", "K3", 'E', 'A')
    );
    assert_eq!(l[0].path_terminator, PathTerminator::IF);
    assert_eq!(
        l[0].altitude_description,
        Some(AltitudeDescription::AtOrAbove)
    );
    assert_eq!(l[0].altitude1, Some(Altitude::Feet(4000)));
    assert_eq!(l[0].transition_altitude, Some(Altitude::Feet(18_000)));
    let c = l[0].center_fix.as_ref().expect("center fix");
    assert_eq!(
        (
            c.ident.as_str(),
            c.icao_region.as_str(),
            c.section,
            c.subsection
        ),
        ("KMSP", "K3", 'P', 'A')
    );
    assert_eq!(l[1].path_terminator, PathTerminator::TF);
    assert_eq!(l[1].description, ['E', 'E', ' ', ' ']);
    assert_eq!(l[2].route_type, '6'); // enroute transition
    assert_eq!(l[2].transition, "DKOTA");
    assert_eq!(l[5].fix.as_ref().map(|f| f.ident.as_str()), Some("HHCEE"));
    assert_eq!(l[5].sequence, 40);
    assert_eq!(l[0].meta.cycle.to_string(), "2611");
}

// REQ: OA-424-004, OA-424-007
#[test]
fn star_legs_group_into_transitions() {
    let cifp = file_of(&["star.txt"]);
    assert!(is_clean(&cifp), "{:?}", cifp.errors);
    assert_eq!(cifp.procedures.len(), 1);
    let p = &cifp.procedures[0];
    assert_eq!(
        (p.kind, p.ident.as_str(), p.airport_ident.as_str()),
        (ProcedureKind::Star, "BENKY6", "KORD")
    );
    let names: Vec<_> = p.routes.iter().map(|r| r.transition.as_str()).collect();
    assert_eq!(names, ["BFORD", "BYLAW", "CASHN", "DRAMS"]);
    assert!(p.routes.iter().all(|r| r.route_type == '4')); // enroute transition
    let bford = &p.routes[0];
    assert_eq!(bford.legs.len(), 3);
    assert_eq!(
        bford.legs[0].fix.as_ref().map(|f| f.ident.as_str()),
        Some("BFORD")
    );
    assert_eq!(bford.legs[0].altitude1, Some(Altitude::FlightLevel(220)));
    assert_eq!(
        bford.legs[0].altitude_description,
        Some(AltitudeDescription::AtOrAbove)
    );
    assert_eq!(bford.legs[2].altitude1, Some(Altitude::Feet(12_000)));
    // `B`: between FL230 (altitude 1) and FL200 (altitude 2).
    let bylaw = &p.routes[1].legs[0];
    assert_eq!(
        bylaw.altitude_description,
        Some(AltitudeDescription::Between)
    );
    assert_eq!(bylaw.altitude1, Some(Altitude::FlightLevel(230)));
    assert_eq!(bylaw.altitude2, Some(Altitude::FlightLevel(200)));
    assert_eq!(bylaw.transition_altitude, Some(Altitude::Feet(18_000)));
    // Legs are in strictly increasing sequence within each route.
    assert!(p
        .routes
        .iter()
        .all(|r| r.legs.windows(2).all(|w| w[0].sequence < w[1].sequence)));
    assert_eq!(procedures_at(&cifp, "KORD", ProcedureKind::Star).count(), 1);
    assert_eq!(procedures_at(&cifp, "KORD", ProcedureKind::Sid).count(), 0);
}

// REQ: OA-424-004, OA-424-007
#[test]
fn approach_legs_decode_every_constraint() {
    let l = legs("approach.txt");
    assert_eq!(l.len(), 19); // 20 records, 1 continuation
    let ils: Vec<_> = l.iter().filter(|x| x.ident == "I04R").collect();
    assert_eq!(ils.len(), 11);
    // Approach transition RAHNN, route type A.
    assert_eq!(
        (ils[0].route_type, ils[0].transition.as_str()),
        ('A', "RAHNN")
    );
    assert_eq!(ils[0].description, ['E', ' ', ' ', 'A']); // IAF: `A` in position 4 (first-fix code)
    assert_eq!(ils[0].altitude1, Some(Altitude::Feet(11_000)));
    assert_eq!(ils[0].route_qualifier_1, Some('N'));
    assert_eq!(ils[0].route_qualifier_2, Some('S'));
    // CF leg with a recommended navaid (the localizer IFJU), theta/rho, course and distance.
    let cf = ils[2];
    assert_eq!(cf.path_terminator, PathTerminator::CF);
    assert_eq!(cf.fix.as_ref().map(|f| f.ident.as_str()), Some("HARRL"));
    assert_eq!(cf.description, ['E', 'E', ' ', 'B']);
    let nav = cf.recommended_navaid.as_ref().expect("navaid");
    assert_eq!(
        (
            nav.ident.as_str(),
            nav.icao_region.as_str(),
            nav.section,
            nav.subsection
        ),
        ("IFJU", "K5", 'P', 'I')
    );
    assert_eq!(cf.theta_deg, Some(224.5));
    assert_eq!(cf.rho_nm, Some(14.9));
    assert_eq!(cf.course_deg, Some(44.5));
    assert_eq!(cf.distance_or_time, Some(DistanceOrTime::Distance(8.1)));
    assert_eq!(cf.altitude1, Some(Altitude::Feet(5000)));
    // Final approach course fix with a glide slope intercept altitude and a vertical angle.
    let faf = ils[5];
    assert_eq!(faf.fix.as_ref().map(|f| f.ident.as_str()), Some("REKKS"));
    assert_eq!(
        faf.altitude_description,
        Some(AltitudeDescription::Other('H'))
    );
    assert_eq!(faf.altitude1, Some(Altitude::Feet(2200)));
    assert_eq!(faf.altitude2, Some(Altitude::Feet(2128)));
    assert_eq!(faf.vertical_angle_deg, Some(-3.0));
    let centre = faf.center_fix.as_ref().expect("MSA centre");
    assert_eq!(
        (centre.ident.as_str(), centre.section, centre.subsection),
        ("KORD", 'P', 'A')
    );
    // Runway fix, then CA, VI legs without a fix, then a VOR-referenced CF and a hold.
    assert_eq!(ils[6].fix.as_ref().map(|f| f.ident.as_str()), Some("RW04R"));
    assert_eq!(
        (
            ils[6].description[0],
            ils[6].description[1],
            ils[6].description[3]
        ),
        ('G', 'Y', 'M')
    );
    assert_eq!(ils[6].altitude1, Some(Altitude::Feet(713))); // threshold crossing altitude
    let ca = ils[7];
    assert_eq!(ca.path_terminator, PathTerminator::CA);
    assert!(ca.fix.is_none());
    assert_eq!(ca.course_deg, Some(44.5));
    assert_eq!(ca.altitude1, Some(Altitude::Feet(1200)));
    assert_eq!(ils[8].path_terminator, PathTerminator::VI);
    assert_eq!(ils[8].course_deg, Some(60.0));
    assert_eq!(
        ils[9]
            .fix
            .as_ref()
            .map(|f| (f.ident.as_str(), f.section, f.subsection)),
        Some(("LAIRD", 'E', 'A'))
    );
    assert_eq!(
        ils[9]
            .recommended_navaid
            .as_ref()
            .map(|n| (n.ident.as_str(), n.section, n.subsection)),
        Some(("OBK", 'D', ' '))
    );
    assert_eq!(ils[9].theta_deg, Some(133.6));
    assert_eq!(ils[9].rho_nm, Some(21.4));
    let hold = ils[10];
    assert_eq!(hold.path_terminator, PathTerminator::HM);
    assert_eq!(hold.turn_direction, Some(TurnDirection::Right));
    assert_eq!(hold.distance_or_time, Some(DistanceOrTime::Time(1.0))); // `T010`: 1.0 minute
    assert_eq!(hold.course_deg, Some(313.6));
    assert_eq!(hold.description, ['E', 'E', ' ', ' ']);
    // RNAV approach legs.
    let r28: Vec<_> = l.iter().filter(|x| x.ident == "R28C").collect();
    assert_eq!(r28.len(), 8);
    assert_eq!(r28[0].rnp_code, None);
    assert_eq!(r28[1].rnp_code.as_deref(), Some("010")); // RNP 1.0
    assert_eq!(r28[3].rnp_code.as_deref(), Some("031")); // RNP 0.3 on the final segment
    assert_eq!(
        r28[3].altitude_description,
        Some(AltitudeDescription::Other('V'))
    );
    assert_eq!(r28[3].altitude1, Some(Altitude::Feet(1260)));
    assert_eq!(r28[3].altitude2, Some(Altitude::Feet(1278)));
    let dpa = r28[6];
    assert_eq!(dpa.path_terminator, PathTerminator::DF);
    assert_eq!(dpa.turn_direction, Some(TurnDirection::Left));
    assert_eq!(
        dpa.fix.as_ref().map(|f| (f.ident.as_str(), f.section)),
        Some(("DPA", 'D'))
    );
    assert_eq!(r28[0].route_qualifier_1, Some('J'));
    assert_eq!(r28[0].gnss_fms_indication, Some('A'));
}

// REQ: OA-424-004
#[test]
fn continuation_records_attach_to_their_primary_leg() {
    let cifp = file_of(&["approach.txt"]);
    assert!(is_clean(&cifp), "{:?}", cifp.errors);
    let r28c = cifp
        .procedures
        .iter()
        .find(|p| p.ident == "R28C")
        .expect("R28C");
    assert_eq!(r28c.routes.len(), 1);
    let seika = r28c.routes[0]
        .legs
        .iter()
        .find(|l| l.sequence == 20)
        .expect("SEIKA");
    assert_eq!(seika.fix.as_ref().map(|f| f.ident.as_str()), Some("SEIKA"));
    assert_eq!(seika.continuations.len(), 1);
    let c = &seika.continuations[0];
    assert_eq!((c.number, c.application_type), ('2', 'W'));
    let los = levels_of_service(c).expect("level of service");
    let want = [(true, "LPV"), (true, "LNAV/VNAV"), (true, "LNAV")];
    assert_eq!(los.len(), 3);
    for (got, (auth, name)) in los.iter().zip(want) {
        assert_eq!((got.authorized, got.name.as_str()), (auth, name));
    }
    // Other legs have none.
    assert_eq!(
        r28c.routes[0]
            .legs
            .iter()
            .filter(|l| !l.continuations.is_empty())
            .count(),
        1
    );
    // The primary and its continuation are consecutive records.
    assert_eq!(c.meta.line, seika.meta.line + 1);
}

fn airspaces(name: &str) -> Vec<Airspace> {
    let cifp = file_of(&[name]);
    assert!(is_clean(&cifp), "{:?}", cifp.errors);
    let mut v = cifp.controlled_airspace;
    v.extend(cifp.special_use_airspace);
    v
}

// REQ: OA-424-005, OA-424-007
#[test]
fn class_b_boundary_has_arcs_with_center_radius_and_bearing() {
    let a = airspaces("uc.txt");
    assert_eq!(a.len(), 2); // ORD Class B areas A and C
    let area_a = &a[0];
    assert_eq!(area_a.kind, AirspaceKind::Controlled);
    assert_eq!(
        (area_a.airspace_type, area_a.classification),
        ('T', Some('B'))
    );
    assert_eq!(area_a.designation, "KORD");
    assert_eq!(area_a.icao_region, "K5");
    assert_eq!(area_a.multiple_code, 'A');
    assert_eq!(area_a.name, "CHICAGO AREA A");
    let lower = area_a.lower.expect("lower");
    assert_eq!(
        (lower.altitude, lower.unit),
        (Altitude::Ground, AltitudeUnit::Agl)
    ); // GND is AGL (FAA readme)
    let upper = area_a.upper.expect("upper");
    assert_eq!(
        (upper.altitude, upper.unit),
        (Altitude::Feet(10_000), AltitudeUnit::Msl)
    );
    assert_eq!(area_a.boundary.len(), 14);
    assert!(is_closed(area_a));
    assert!(area_a.boundary.last().is_some_and(|p| p.end));
    assert!(area_a
        .boundary
        .windows(2)
        .all(|w| w[0].sequence < w[1].sequence));
    // First point: a clockwise arc around a centre 5.0 NM away at bearing 349.4 degrees.
    let p = &area_a.boundary[0];
    assert_eq!(p.via, BoundaryVia::ClockwiseArc);
    assert_eq!(p.arc_distance_nm, Some(5.0));
    assert_eq!(p.arc_bearing_deg, Some(349.4));
    let (point, origin) = (p.position.expect("point"), p.arc_origin.expect("origin"));
    assert!(close(
        origin,
        41.0 + 59.0 / 60.0 + 16.0 / 3600.0,
        -(87.0 + 54.0 / 60.0 + 17.0 / 3600.0)
    ));
    // Independent geometry: the arc point lies 5.0 NM from its origin at bearing 349.4.
    let (d, b) = distance_bearing(origin, point);
    assert!((d - 5.0).abs() < 0.02, "distance {d}");
    assert!((b - 349.4).abs() < 0.1, "bearing {b}");
    assert_eq!(area_a.boundary[1].via, BoundaryVia::GreatCircle);
    assert!(area_a.boundary[1].arc_origin.is_none());
    assert_eq!(area_a.boundary[2].via, BoundaryVia::ClockwiseArc);
    let (d2, b2) = {
        let q = &area_a.boundary[2];
        distance_bearing(q.arc_origin.expect("origin"), q.position.expect("point"))
    };
    assert!(
        (d2 - 6.0).abs() < 0.05 && (b2 - 90.1).abs() < 0.1,
        "{d2} {b2}"
    );
    assert_eq!(area_a.boundary[2].arc_distance_nm, Some(6.0));
    assert_eq!(area_a.boundary[2].arc_bearing_deg, Some(90.1));
}

// REQ: OA-424-005, OA-424-007
#[test]
fn special_use_airspace_has_circles_continuations_and_limits() {
    let a = airspaces("ur.txt");
    assert_eq!(a.len(), 3);
    let boardman = &a[0];
    assert_eq!(boardman.kind, AirspaceKind::SpecialUse);
    assert_eq!(
        (boardman.airspace_type, boardman.designation.as_str()),
        ('M', "BOARDMAN")
    ); // MOA
    assert_eq!(boardman.name, "BOARDMAN MOA");
    assert_eq!(boardman.level, Some('L'));
    assert_eq!(
        boardman.lower.map(|l| (l.altitude, l.unit)),
        Some((Altitude::Feet(4000), AltitudeUnit::Msl))
    );
    assert_eq!(
        boardman.upper.map(|l| (l.altitude, l.unit)),
        Some((Altitude::Feet(17_999), AltitudeUnit::Msl))
    );
    assert_eq!(boardman.boundary.len(), 4);
    assert_eq!(controlling_agency(boardman), Some("FAA SEATTLE ARTCC"));
    assert_eq!(boardman.continuations.len(), 1);
    assert_eq!(boardman.continuations[0].application_type, 'C');
    assert!(!is_closed(boardman)); // the fixture is cut before the closing record

    // A circle: via `C` with the `E` end flag, centre and radius, no boundary point.
    let circle = &a[1];
    assert_eq!(
        (circle.airspace_type, circle.designation.as_str()),
        ('A', "680")
    ); // Alert A-680
    assert_eq!(circle.name, "A-680");
    assert_eq!(circle.boundary.len(), 1);
    let p = &circle.boundary[0];
    assert_eq!(p.via, BoundaryVia::Circle);
    assert!(p.end && p.position.is_none());
    assert_eq!(p.arc_distance_nm, Some(3.0));
    assert!(close(
        p.arc_origin.expect("centre"),
        48.0 + 10.0 / 60.0 + 59.0 / 3600.0,
        -(122.0 + 38.0 / 60.0 + 5.0 / 3600.0)
    ));
    assert!(is_closed(circle));
    assert_eq!(
        circle.lower.map(|l| (l.altitude, l.unit)),
        Some((Altitude::Ground, AltitudeUnit::Agl))
    );
    assert_eq!(
        circle.upper.map(|l| (l.altitude, l.unit)),
        Some((Altitude::Feet(3000), AltitudeUnit::Msl))
    );

    let r3401a = &a[2];
    assert_eq!(
        (r3401a.airspace_type, r3401a.designation.as_str()),
        ('R', "3401A")
    ); // Restricted R-3401A
    assert_eq!(r3401a.level, Some('B'));
    assert_eq!(
        r3401a.upper.map(|l| l.altitude),
        Some(Altitude::Feet(40_000))
    );
    assert_eq!(controlling_agency(r3401a), Some("FAA INDIANAPOLIS ARTCC"));
    assert_eq!(r3401a.multiple_code, 'A');
}

fn mora_rows() -> Vec<GridMora> {
    records("mora.txt")
        .into_iter()
        .map(|r| match r {
            Record::GridMora(x) => x,
            o => panic!("not a MORA row: {o:?}"),
        })
        .collect()
}

// REQ: OA-424-006, OA-424-007
#[test]
fn grid_mora_rows_decode_and_locate_cells() {
    let rows = mora_rows();
    assert_eq!(rows.len(), 4);
    let n41 = rows
        .iter()
        .find(|r| r.start_lat == 41 && r.start_lon == -90)
        .expect("N41W090");
    assert_eq!(n41.meta.customer_area, ""); // AS records carry no customer code
    assert_eq!(n41.cells[0], Some(32));
    assert_eq!(n41.cells[2], Some(35));
    assert_eq!(n41.cells[20], Some(17));
    assert_eq!(n41.cells[21], None); // UNK: open water
    assert_eq!(n41.cells.iter().flatten().count(), 21);
    // 41 N 88 W: the cell [-88, -87] west to east from -90 is index 2, 3500 ft.
    assert_eq!(mora_value_at(n41, 41.0, -88.0), Some(3500));
    assert_eq!(mora_value_at(n41, 41.999, -87.001), Some(3500));
    assert_eq!(mora_value_at(n41, 42.0, -88.0), None); // next row
    assert_eq!(mora_value_at(n41, 41.5, -60.0), None); // beyond the row
    assert_eq!(mora_value_at(n41, 41.5, -90.5), None);
    assert_eq!(mora_value_at(n41, f64::NAN, -88.0), None);
    // Catskills (Slide Mountain, 42.0 N 74.4 W) sit in cell index 15 of the N41 row: 6500 ft.
    assert_eq!(mora_value_at(n41, 41.99, -74.4), Some(6500));
    // Mount Whitney (36.58 N 118.29 W): the highest cell in the N36W120 row is index 1,
    // [-119, -118], which holds the Sierra Nevada crest.
    let n36 = rows.iter().find(|r| r.start_lat == 36).expect("N36W120");
    assert_eq!(n36.start_lon, -120);
    assert_eq!(mora_value_at(n36, 36.578, -118.292), Some(16_800));
    assert_eq!(n36.cells.iter().flatten().max(), Some(&168));
    // Southern hemisphere row with every cell unknown.
    let s11 = rows.iter().find(|r| r.start_lat == -11).expect("S11W180");
    assert_eq!(s11.start_lon, -180);
    assert!(s11.cells.iter().all(Option::is_none));
    let n44 = rows.iter().find(|r| r.start_lat == 44).expect("N44W090");
    assert_eq!(mora_value_at(n44, 44.1127, -73.9237), Some(7600)); // Mount Marcy's cell, index 16
}
