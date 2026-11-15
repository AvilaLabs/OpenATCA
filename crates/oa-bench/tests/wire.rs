//! `oa-truth` wired into `oa-bench`: the query and leg adapter, the airspace lookups from a
//! model, and the whole binary on a hand-made ADS-B clip and SFDPS fixture whose cross-track
//! error is known. Every identifier is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::process::Command;

use oa_adsb::{encode_payload, reports_len, reports_push, Reports, ROW_EMPTY};
use oa_airspace::model::{
    boundary_source, empty_model, point_kind, stratum, Meta, PosE7, Span, StrIx, VertexIx,
};
use oa_bench::adsb_truth::{build, config_with, legs, map_query, query, truth_section};
use oa_bench::airspace_env::lookup_from_model;
use oa_bench::input::load_adsb;
use oa_bench::truth::NoTruth;
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_sha256::{sha256, to_hex};
use oa_truth::{TruthQuery, TruthSample};

/// 2030-01-02T03:00:00Z, Unix seconds.
const T0_S: i64 = 1_893_553_200;
const LAT0: f64 = 41.0;
const LON0: f64 = -88.0;
const M_PER_DEG: f64 = 111_195.0;
const SPEED_MPS: f64 = 100.0;
/// ERAM is this far north of the eastbound truth, so its cross-track error is this far left.
const OFFSET_M: f64 = 500.0;
const ADDR: u32 = 0xA1B2C3;

fn lon_at(t_s: f64) -> f64 {
    LON0 + SPEED_MPS * t_s / (M_PER_DEG * LAT0.to_radians().cos())
}

/// One aircraft flying due east at 100 m/s, a report per second from -5 s to 125 s.
fn clip_reports(callsigns: &[&[u8; 8]]) -> Reports {
    let mut r = Reports::default();
    for k in -5..=125i64 {
        let mut row = ROW_EMPTY;
        row.icao = ADDR;
        row.time_ns = (T0_S + k) * 1_000_000_000;
        row.lat_e7 = (LAT0 * 1e7).round() as i32;
        row.lon_e7 = (lon_at(k as f64) * 1e7).round() as i32;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        row.source = 0;
        row.baro_alt_ft = 35_000;
        if let Some(c) = callsigns.get(k.rem_euclid(callsigns.len().max(1) as i64) as usize) {
            row.callsign = **c;
        }
        reports_push(&mut r, &row);
    }
    r
}

fn oaf_bytes(r: &Reports) -> Vec<u8> {
    let n = reports_len(r);
    let mut bytes = Vec::new();
    let (mut at, mut seq) = (0usize, 0u64);
    while at < n {
        let end = (at + 100).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    bytes
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-bench-wire-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn iso(t_s: i64) -> String {
    let s = t_s - T0_S;
    format!("2030-01-02T03:{:02}:{:02}.000Z", s / 60, s % 60)
}

/// A track message for the flight at second `t` of the clip, `OFFSET_M` north of the truth.
fn th(t: i64) -> String {
    let lat = LAT0 + OFFSET_M / M_PER_DEG;
    let lon = lon_at(t as f64);
    let (ts, pt) = (iso(T0_S + t + 2), iso(T0_S + t));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0"><message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="{ts}"><enRoute xsi:type="ns5:NasEnRouteType"><position xsi:type="ns5:NasAircraftPositionType" positionTime="{pt}" reportSource="SURVEILLANCE"><actualSpeed><surveillance uom="KNOTS">194.4</surveillance></actualSpeed><altitude uom="FEET">35000.0</altitude><position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>{lat:.6} {lon:.6}</pos></location></position><trackVelocity><x uom="KNOTS">194.4</x><y uom="KNOTS">0.0</y></trackVelocity></position></enRoute><flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="946" siteSpecificPlanId="237" aircraftIdentification="TST123"/><flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="ACTIVE"/><gufi codeSpace="urn:uuid">11111111-2222-4333-8444-555555555555</gufi><aircraftDescription aircraftAddress="A1B2C3" registration="N1"/><flightPlan identifier="KR000001Aa"/></flight></message></ns5:MessageCollection>"#
    )
}

/// Writes the clip and the SFDPS directory (a track message every 5 s from 10 s to 110 s).
fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let d = scratch(name);
    let clip = d.join("clip.oaf");
    std::fs::write(&clip, oaf_bytes(&clip_reports(&[b"TST123\0\0"]))).unwrap();
    let sf = d.join("sfdps");
    std::fs::create_dir_all(&sf).unwrap();
    for (i, t) in (10..=110i64).step_by(5).enumerate() {
        std::fs::write(sf.join(format!("{i:03}.msg")), th(t)).unwrap();
    }
    (d, clip, sf)
}

fn stat(csv: &str, metric: &str, label: &str) -> Vec<f64> {
    let prefix = format!("stat,{metric},{label},");
    let l = csv
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {prefix} in report"));
    l[prefix.len()..]
        .split(',')
        .map(|x| x.parse::<f64>().unwrap())
        .collect()
}

fn run(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_oa-bench"))
        .args(args)
        .output()
        .unwrap()
}

// REQ: OA-BENCH-028
#[test]
fn truth_refusals_map_by_meaning_and_samples_keep_their_values() {
    assert_eq!(
        map_query(TruthQuery::UnknownAircraft),
        Err(NoTruth::UnknownAircraft)
    );
    // The same names, with the same meanings, in both crates.
    assert_eq!(
        map_query(TruthQuery::OutsideLegs),
        Err(NoTruth::OutsideLegs)
    );
    assert_eq!(
        map_query(TruthQuery::NoNearbyReport),
        Err(NoTruth::NoNearbyReport)
    );
    assert_eq!(
        map_query(TruthQuery::TooUncertain),
        Err(NoTruth::TooUncertain)
    );
    let s = TruthSample {
        lat_deg: 41.5,
        lon_deg: -88.5,
        baro_alt_ft: Some(35_000.4),
        vel_east_mps: 100.0,
        vel_north_mps: -2.0,
        vert_rate_fpm: Some(-640.6),
        sigma95_h_m: 7.0,
        leg: oa_truth::LegIx(3),
    };
    let m = map_query(TruthQuery::Ok(s)).unwrap();
    assert_eq!(
        (m.baro_alt_ft, m.vert_rate_fpm, m.leg),
        (Some(35_000), Some(-641), 3)
    );
    assert_eq!((m.lat, m.lon, m.sigma95_h_m), (41.5, -88.5, 7.0));
    // Against a real truth: an unknown address, a time before the leg, a time inside it.
    let r = clip_reports(&[b"TST123\0\0"]);
    let b = build(&r, &config_with(None, None)).unwrap();
    assert_eq!(
        query(&b.truth, 0x123456, T0_S * 1000),
        Err(NoTruth::UnknownAircraft)
    );
    assert_eq!(
        query(&b.truth, ADDR, (T0_S - 600) * 1000),
        Err(NoTruth::OutsideLegs)
    );
    let ok = query(&b.truth, ADDR, (T0_S + 60) * 1000).unwrap();
    assert!((ok.lat - LAT0).abs() < 1e-5 && (ok.vel_east_mps - SPEED_MPS).abs() < 0.5);
}

// REQ: OA-BENCH-029
#[test]
fn leg_callsign_is_the_most_frequent_of_its_reports() {
    // 131 reports; the callsigns rotate TST123, TST123, OTHER (ties impossible: 87 against 44).
    let r = clip_reports(&[b"TST123\0\0", b"TST123\0\0", b"OTHER\0\0\0"]);
    let b = build(&r, &config_with(None, None)).unwrap();
    assert_eq!(b.legs.len(), 1);
    let l = b.legs[0];
    assert_eq!(l.addr, ADDR);
    assert_eq!(l.callsign, Some(*b"TST123\0\0"));
    assert_eq!(l.start_ms, (T0_S - 5) * 1000);
    assert_eq!(l.end_ms, (T0_S + 125) * 1000);
    // No callsign anywhere: none, not an invented one.
    let none = clip_reports(&[]);
    let (t, _) = oa_truth::build_truth(&none, &config_with(None, None)).unwrap();
    assert_eq!(legs(&t, &none)[0].callsign, None);
}

// REQ: OA-BENCH-030
#[test]
fn gate_overrides_change_the_truth_config_and_the_section_records_them() {
    let c = config_with(Some(4.0), Some(20.0));
    assert_eq!((c.bracket_ms, c.max_sigma95_h_m), (4000, 20.0));
    let d = config_with(None, None);
    assert_eq!((d.bracket_ms, d.max_sigma95_h_m), (10_000, 50.0));
    let b = build(&clip_reports(&[b"TST123\0\0"]), &c).unwrap();
    let sec = truth_section(&b, &c);
    let get = |k: &str| sec.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(get("bracket_s").as_deref(), Some("4"));
    assert_eq!(get("max_sigma95_h_m").as_deref(), Some("20"));
    assert_eq!(get("rows_in").as_deref(), Some("131"));
    assert_eq!(get("legs").as_deref(), Some("1"));
    assert_eq!(get("truth_digest"), Some(b.digest.clone()));
    assert!(get("loo_cross_track_m").unwrap().starts_with("n="));
}

// REQ: OA-BENCH-031
#[test]
fn the_clip_is_read_and_hashed() {
    let (d, clip, _) = fixture("clip");
    let l = load_adsb(&clip).unwrap();
    assert_eq!((reports_len(&l.reports), l.bad_frames), (131, 0));
    assert_eq!(l.hash.files, 1);
    assert_eq!(
        l.hash.sha256,
        to_hex(&sha256(&std::fs::read(&clip).unwrap()))
    );
    assert!(load_adsb(&d.join("missing.oaf")).is_err());
    let _ = std::fs::remove_dir_all(d);
}

fn model_with_zone() -> oa_airspace::model::Model {
    let mut m = empty_model(Meta {
        nasr_effective: 20261029,
        cifp_cycle: 2611,
        artcc: "ZAA".to_string(),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    });
    let (fixx, apt, zaa, zbb) = (
        s_t(&mut m, "FIXXX"),
        s_t(&mut m, "KAAA"),
        s_t(&mut m, "ZAA"),
        s_t(&mut m, "ZBB"),
    );
    let faa = s_t(&mut m, "AAA");
    let none = StrIx::NONE;
    // (kind, lat, lon, elev, ident, alt_ident, scope)
    let pts = [
        (point_kind::FIX, 41.5, -88.0, i32::MIN, fixx, none, none),
        (point_kind::AIRPORT, 41.2, -88.2, 650, apt, faa, none),
        (
            point_kind::THRESHOLD,
            41.21,
            -88.21,
            i32::MIN,
            s_t(&mut m, "RW09"),
            none,
            apt,
        ),
    ];
    for (k, la, lo, el, id, alt, sc) in pts {
        m.points.kind.push(k);
        m.points.lat.push((la * 1e7_f64).round() as i32);
        m.points.lon.push((lo * 1e7_f64).round() as i32);
        m.points.elev_ft.push(el);
        m.points.ident.push(id);
        m.points.alt_ident.push(alt);
        m.points.region.push(none);
        m.points.scope.push(sc);
        m.points.name.push(none);
    }
    // ZAA: the square 41..42 N, 88..87 W; ZBB: the square east of it.
    for (fac, lon_w, lon_e) in [(zaa, -89.0, -88.0), (zbb, -88.0, -87.0)] {
        let start = m.vertices.len() as u32;
        for (la, lo) in [(41.0, lon_w), (41.0, lon_e), (42.0, lon_e), (42.0, lon_w)] {
            m.vertices.push(PosE7 {
                lat: (la * 1e7_f64).round() as i32,
                lon: (lo * 1e7_f64).round() as i32,
            });
        }
        m.boundaries.source.push(boundary_source::NASR_ARB);
        m.boundaries.stratum.push(stratum::ARTCC_HIGH);
        m.boundaries.facility.push(fac);
        m.boundaries.vertices.push(Span {
            start: VertexIx::new(start),
            len: 4,
        });
    }
    m
}

fn s_t(m: &mut oa_airspace::model::Model, t: &str) -> StrIx {
    m.strings.text.push_str(t);
    m.strings.offsets.push(m.strings.text.len() as u32);
    StrIx::new((m.strings.offsets.len() - 2) as u32)
}

// REQ: OA-BENCH-032
#[test]
fn the_airspace_lookups_come_from_the_model() {
    let l = lookup_from_model(&model_with_zone());
    assert_eq!(l.fix("FIXXX"), Some((41.5, -88.0)));
    assert_eq!(l.fix("KAAA"), None, "an airport is not a fix");
    assert_eq!(l.fix("NOPE"), None);
    let a = l.airport("KAAA").unwrap();
    assert_eq!(a.elev_ft, 650);
    assert_eq!(a.thresholds, vec![(41.21, -88.21)]);
    assert_eq!(
        l.airport("AAA").unwrap().thresholds.len(),
        1,
        "FAA identifier"
    );
    assert!(l.in_zone(41.5, -88.5) && !l.in_zone(41.5, -87.5) && !l.in_zone(43.0, -88.5));
    assert_eq!(l.unit_at(41.5, -88.5), Some(*b"ZAA\0\0\0\0\0"));
    assert_eq!(l.unit_at(41.5, -87.5), Some(*b"ZBB\0\0\0\0\0"));
    assert_eq!(l.unit_at(45.0, -88.5), None);
    assert!(l.has_zone());
    assert_eq!(l.counts(), (1, 2, 2));
}

fn push_fix(m: &mut oa_airspace::model::Model, ident: &str, lat: f64, lon: f64, region: &str) {
    let (id, rg) = (s_t(m, ident), s_t(m, region));
    m.points.kind.push(point_kind::FIX);
    m.points.lat.push((lat * 1e7_f64).round() as i32);
    m.points.lon.push((lon * 1e7_f64).round() as i32);
    m.points.elev_ft.push(i32::MIN);
    m.points.ident.push(id);
    m.points.alt_ident.push(StrIx::NONE);
    m.points.region.push(rg);
    m.points.scope.push(StrIx::NONE);
    m.points.name.push(StrIx::NONE);
}

// REQ: OA-BENCH-051
#[test]
fn an_identifier_naming_two_places_is_ambiguous_not_the_first_row() {
    let mut m = model_with_zone();
    push_fix(&mut m, "DUPX", 41.2, -88.1, "K2");
    push_fix(&mut m, "DUPX", 44.0, -80.0, "K7");
    // The same navaid listed twice at one place is one place.
    push_fix(&mut m, "SAMEE", 41.3, -88.3, "K2");
    push_fix(&mut m, "SAMEE", 41.3, -88.3, "K2");
    let l = lookup_from_model(&m);
    assert_eq!(l.fix("DUPX"), None, "two places: no single answer");
    let two = l.fix_all("DUPX");
    assert_eq!(two.len(), 2);
    assert!((two[0].0 - 41.2).abs() < 1e-6 && (two[1].0 - 44.0).abs() < 1e-6);
    assert_eq!(l.fix("SAMEE"), Some((41.3, -88.3)));
    assert_eq!(l.fix_all("SAMEE").len(), 1);
    assert_eq!(l.fix("FIXXX"), Some((41.5, -88.0)));
    assert_eq!(l.ambiguous_fixes, 1);
    assert!(l.fix_all("NOPE").is_empty());
}

// REQ: OA-BENCH-033
#[test]
fn the_binary_scores_a_known_cross_track_error_end_to_end() {
    let (d, clip, sf) = fixture("e2e");
    let out = d.join("out");
    let args: Vec<&std::ffi::OsStr> = vec![
        "--sfdps-dir".as_ref(),
        sf.as_os_str(),
        "--adsb".as_ref(),
        clip.as_os_str(),
        "--out".as_ref(),
        out.as_os_str(),
    ];
    let o = run(&args);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let csv = std::fs::read_to_string(out.join("report.csv")).unwrap();
    let txt = std::fs::read_to_string(out.join("report.txt")).unwrap();
    // 21 track messages, all associated by address and paired.
    let t1 = stat(&csv, "T1", "all");
    assert_eq!(t1[0], 21.0, "{csv}");
    // ERAM is 500 m left of an eastbound truth: cross-track -500 m (positive is right). The
    // tolerance covers the six-decimal position text and the flat-earth conversion.
    assert!((t1[1] + OFFSET_M).abs() < 2.0, "mean {}", t1[1]);
    assert!((t1[2] - OFFSET_M).abs() < 2.0, "rms {}", t1[2]);
    // Altitude and speed agree by construction.
    let t4 = stat(&csv, "T4", "all");
    assert!(t4[1].abs() < 1.0, "altitude error {}", t4[1]);
    assert!(csv.contains("associated by address,"), "{csv}");
    // The report carries the truth section, the leave-one-out table and the digest, the clip's
    // hash and the gate values.
    assert!(
        txt.contains("Truth (ADS-B reference trajectories)"),
        "{txt}"
    );
    assert!(
        txt.contains("rows_kept: 131") && txt.contains("legs: 1"),
        "{txt}"
    );
    assert!(txt.contains("loo_cross_track_m: n="), "{txt}");
    assert!(txt.contains("truth_digest: "), "{txt}");
    assert!(txt.contains("truth gates: the design's values"), "{txt}");
    assert!(txt.contains("airspace: no model given"), "{txt}");
    let h = to_hex(&sha256(&std::fs::read(&clip).unwrap()));
    assert!(txt.contains(&format!("sha256 {h}")), "{txt}");
    assert!(csv.contains("\ntruth,,truth_digest,"), "{csv}");
    // A repeat gives the same bytes.
    let out2 = d.join("out2");
    let mut args2 = args.clone();
    args2[5] = out2.as_os_str();
    assert!(run(&args2).status.success());
    for f in ["report.txt", "report.csv"] {
        assert_eq!(
            std::fs::read(out.join(f)).unwrap(),
            std::fs::read(out2.join(f)).unwrap(),
            "{f}"
        );
    }
    // A tightened gate is recorded with its reason, and a gate under the truth's own
    // uncertainty leaves no pairs.
    let out3 = d.join("out3");
    let mut args3 = args.clone();
    args3[5] = out3.as_os_str();
    let extra: Vec<&std::ffi::OsStr> = vec![
        "--truth-max-sigma-m".as_ref(),
        "0.5".as_ref(),
        "--truth-bracket-s".as_ref(),
        "6".as_ref(),
        "--truth-reason".as_ref(),
        "tuned from the leave-one-out table".as_ref(),
        "--no-loo".as_ref(),
    ];
    args3.extend(extra);
    let o3 = run(&args3);
    assert!(
        o3.status.success(),
        "{}",
        String::from_utf8_lossy(&o3.stderr)
    );
    let t3 = std::fs::read_to_string(out3.join("report.txt")).unwrap();
    assert!(t3.contains("truth gates OVERRIDDEN: bracket 6 s"), "{t3}");
    assert!(t3.contains("0.5 m (design 50 m)"), "{t3}");
    assert!(
        t3.contains("reason: tuned from the leave-one-out table"),
        "{t3}"
    );
    assert!(t3.contains("loo: not run"), "{t3}");
    assert!(t3.contains("bracket_s: 6"), "{t3}");
    let _ = std::fs::remove_dir_all(d);
}

// REQ: OA-BENCH-034
#[test]
fn real_clip_with_no_overlap_gives_no_pairs_and_a_full_exclusions_table() {
    let (Some(data), Some(clip)) = (
        std::env::var_os("OPENATCA_DATA"),
        std::env::var_os("OPENATCA_ADSB_CLIP"),
    ) else {
        eprintln!("OPENATCA_DATA / OPENATCA_ADSB_CLIP not set: skipping the real-data smoke run");
        return;
    };
    let sf = std::fs::read_dir(PathBuf::from(&data).join("scds"))
        .ok()
        .and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("sfdps-"))
            })
        });
    let Some(sf) = sf else {
        eprintln!("no SFDPS sample under OPENATCA_DATA/scds: skipping");
        return;
    };
    let out = scratch("real");
    let o = run(&[
        "--sfdps-dir".as_ref(),
        sf.as_os_str(),
        "--adsb".as_ref(),
        clip.as_os_str(),
        "--out".as_ref(),
        out.as_os_str(),
        "--no-loo".as_ref(),
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let txt = std::fs::read_to_string(out.join("report.txt")).unwrap();
    assert!(txt.contains("Paired samples: 0"), "{txt}");
    assert!(txt.contains("Exclusions"), "{txt}");
    let _ = std::fs::remove_dir_all(out);
}
