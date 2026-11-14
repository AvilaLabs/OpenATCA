//! Integration tests over the full FAA NASR files. They read the directory named by the
//! `OPENATCA_DATA` environment variable (the extracted CSV directory) and skip when it is unset.
//! Run with `--nocapture` to see the schema-violation and DMS-discrepancy report.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nasr::schema::{self, parse_schemas, validate, Schemas, Validation, ViolationKind};
use oa_nasr::{apt, arb, awy, cls, fix, nav, open, position, procedure, Parsed, Position};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn data_dir() -> Option<PathBuf> {
    match std::env::var_os("OPENATCA_DATA") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => {
            eprintln!("OPENATCA_DATA is not set; skipping real-data test");
            None
        }
    }
}

fn file(dir: &std::path::Path, name: &str) -> (String, std::io::BufReader<std::fs::File>) {
    let path = dir.join(format!("{name}.csv"));
    let reader = open(&path).unwrap_or_else(|e| panic!("{e}"));
    (format!("{name}.csv"), reader)
}

fn schemas(dir: &std::path::Path) -> Schemas {
    let mut all = Schemas::default();
    for s in ["ARB", "FIX", "NAV", "APT", "AWY", "STAR", "DP", "CLS_ARSP"] {
        let (n, r) = file(dir, &format!("{s}_CSV_DATA_STRUCTURE"));
        let part = parse_schemas(&n, r).unwrap_or_else(|e| panic!("{e}"));
        all.files.extend(part.files);
        all.warnings.extend(part.warnings);
    }
    all
}

const FILES: [&str; 17] = [
    "ARB_BASE",
    "ARB_SEG",
    "FIX_BASE",
    "FIX_NAV",
    "NAV_BASE",
    "APT_BASE",
    "APT_RWY",
    "APT_RWY_END",
    "AWY_BASE",
    "AWY_SEG_ALT",
    "STAR_BASE",
    "STAR_APT",
    "STAR_RTE",
    "DP_BASE",
    "DP_APT",
    "DP_RTE",
    "CLS_ARSP",
];

fn validations(dir: &std::path::Path) -> Vec<Validation> {
    let schemas = schemas(dir);
    FILES
        .iter()
        .map(|f| {
            let schema = schemas
                .get(f)
                .unwrap_or_else(|| panic!("no schema for {f}"));
            let (n, r) = file(dir, f);
            validate(schema, &n, r).unwrap_or_else(|e| panic!("{e}"))
        })
        .collect()
}

fn kind_name(k: &ViolationKind) -> &'static str {
    match k {
        ViolationKind::MissingColumn => "MissingColumn",
        ViolationKind::UnexpectedColumn => "UnexpectedColumn",
        ViolationKind::TooLong { .. } => "TooLong",
        ViolationKind::NotNumeric(_) => "NotNumeric",
        ViolationKind::TooManyIntegerDigits { .. } => "TooManyIntegerDigits",
        ViolationKind::TooManyFractionDigits { .. } => "TooManyFractionDigits",
        ViolationKind::NullViolation => "NullViolation",
    }
}

// REQ: OA-NASR-001
#[test]
fn real_schema_validation_report() {
    let Some(dir) = data_dir() else { return };
    let vals = validations(&dir);
    for v in &vals {
        let mut by_kind: BTreeMap<(&str, &str), u64> = BTreeMap::new();
        for x in &v.violations {
            *by_kind
                .entry((kind_name(&x.kind), x.column.as_str()))
                .or_default() += 1;
        }
        println!(
            "{}: {} rows, {} violations",
            v.file,
            v.rows,
            schema::total_violations(v)
        );
        for ((k, c), n) in &by_kind {
            println!("    {k} {c}: {n}");
        }
        for x in v.violations.iter().take(3) {
            println!("    e.g. {x:?}");
        }
    }
    // Every file has rows, and every expected column is present.
    for v in &vals {
        assert!(v.rows > 0, "{} has no rows", v.file);
        assert!(
            !v.violations
                .iter()
                .any(|x| x.kind == ViolationKind::MissingColumn),
            "{}: schema column missing from file",
            v.file
        );
        assert_eq!(v.dropped, 0);
    }
}

fn max_diff<'a>(it: impl Iterator<Item = &'a Position>) -> (f64, f64) {
    it.fold((0.0, 0.0), |(a, b), p| {
        (a.max(p.lat_diff), b.max(p.lon_diff))
    })
}

fn report<T>(name: &str, p: &Parsed<T>, diff: (f64, f64)) {
    println!(
        "{name}: {} records, {} rejected, max |dLat| {:e}, max |dLon| {:e}",
        p.records.len(),
        p.rejected.len(),
        diff.0,
        diff.1
    );
    for e in p.rejected.iter().take(3) {
        println!("    rejected: {e}");
    }
}

// REQ: OA-NASR-002
#[test]
fn real_artcc_boundaries() {
    let Some(dir) = data_dir() else { return };
    let (n, r) = file(&dir, "ARB_BASE");
    let base = arb::parse_base(&n, r).unwrap_or_else(|e| panic!("{e}"));
    report(
        "ARB_BASE",
        &base,
        max_diff(base.records.iter().map(|b| &b.position)),
    );
    assert!(base.rejected.is_empty());
    let zau = base
        .records
        .iter()
        .find(|b| b.location_id == "ZAU")
        .expect("ZAU facility");
    assert_eq!(zau.location_type, "ARTCC");
    assert_eq!(zau.icao_id.as_deref(), Some("KZAU"));

    let (n, r) = file(&dir, "ARB_SEG");
    let segs = arb::parse_boundaries(&n, r).unwrap_or_else(|e| panic!("{e}"));
    let pts: Vec<&Position> = segs
        .records
        .iter()
        .flat_map(|b| b.points.iter().map(|p| &p.position))
        .collect();
    report("ARB_SEG", &segs, max_diff(pts.into_iter()));
    assert!(segs.rejected.is_empty());
    let get = |alt: &str| {
        segs.records
            .iter()
            .find(|b| b.location_id == "ZAU" && b.altitude == alt && b.boundary_type == "ARTCC")
            .unwrap_or_else(|| panic!("ZAU {alt} boundary"))
    };
    let (high, low) = (get("HIGH"), get("LOW"));
    assert_eq!(high.points.len(), 31);
    assert_eq!(low.points.len(), 34);
    for b in &segs.records {
        assert!(b.points.windows(2).all(|w| w[0].point_seq < w[1].point_seq));
        assert!(
            b.points.len() >= 3,
            "{} {} has {} points",
            b.location_id,
            b.altitude,
            b.points.len()
        );
    }
    assert_eq!(
        low.points.last().and_then(|p| p.description.as_deref()),
        Some("TO POINT OF BEGINNING")
    );
}

// REQ: OA-NASR-003
#[test]
fn real_fixes_navaids_airports() {
    let Some(dir) = data_dir() else { return };
    let (n, r) = file(&dir, "FIX_BASE");
    let fixes = fix::parse_fixes(&n, r).unwrap_or_else(|e| panic!("{e}"));
    report(
        "FIX_BASE",
        &fixes,
        max_diff(fixes.records.iter().map(|f| &f.position)),
    );
    assert!(fixes.records.len() > 50_000);
    assert!(fixes
        .records
        .iter()
        .all(|f| position::max_diff(&f.position) < 1e-7));
    assert!(fixes
        .records
        .iter()
        .any(|f| f.fix_id == "AAALL" && f.icao_region_code == "K6"));

    let (n, r) = file(&dir, "FIX_NAV");
    let fnav = fix::parse_fix_navs(&n, r).unwrap_or_else(|e| panic!("{e}"));
    report("FIX_NAV", &fnav, (0.0, 0.0));
    assert!(fnav.records.len() > 10_000);

    let (n, r) = file(&dir, "NAV_BASE");
    let navs = nav::parse_navaids(&n, r).unwrap_or_else(|e| panic!("{e}"));
    let dme = navs
        .records
        .iter()
        .filter_map(|x| x.tacan_dme_position.as_ref());
    let d = max_diff(navs.records.iter().map(|x| &x.position));
    let d2 = max_diff(dme);
    report("NAV_BASE", &navs, (d.0.max(d2.0), d.1.max(d2.1)));
    assert!(navs.records.len() > 1_000);
    // The FAA publishes 8 decimal places, truncated: DMS and decimal agree to within 1e-8
    // degrees everywhere except one navaid (TDD, whose decimal is the DME antenna's).
    let off: Vec<_> = navs
        .records
        .iter()
        .filter(|x| position::max_diff(&x.position) > 1e-7)
        .collect();
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].nav_id, "TDD");
    println!(
        "NAV_BASE DMS/decimal outlier: {} line {}",
        off[0].nav_id, off[0].line
    );
    assert!(navs.records.iter().any(|x| x.nav_id == "DPA"
        && x.nav_type.starts_with("VOR")
        && x.state_code.as_deref() == Some("IL")));

    let (n, r) = file(&dir, "APT_BASE");
    let apts = apt::parse_airports(&n, r).unwrap_or_else(|e| panic!("{e}"));
    report(
        "APT_BASE",
        &apts,
        max_diff(apts.records.iter().map(|a| &a.position)),
    );
    let ord = apts
        .records
        .iter()
        .find(|a| a.arpt_id == "ORD")
        .expect("ORD");
    assert_eq!(ord.site_type_code, "A");
    assert_eq!(ord.resp_artcc_id, "ZAU");

    let (n, r) = file(&dir, "APT_RWY");
    let rwys = apt::parse_runways(&n, r).unwrap_or_else(|e| panic!("{e}"));
    report("APT_RWY", &rwys, (0.0, 0.0));
    let ord_rwys: Vec<_> = rwys
        .records
        .iter()
        .filter(|x| x.site_no == ord.site_no)
        .collect();
    assert!(ord_rwys.len() >= 6, "ORD has {} runways", ord_rwys.len());
    assert!(ord_rwys.iter().any(|x| x.rwy_id == "10L/28R"));

    let (n, r) = file(&dir, "APT_RWY_END");
    let ends = apt::parse_runway_ends(&n, r).unwrap_or_else(|e| panic!("{e}"));
    let a = max_diff(ends.records.iter().filter_map(|e| e.position.as_ref()));
    let b = max_diff(
        ends.records
            .iter()
            .filter_map(|e| e.displaced_thr_position.as_ref()),
    );
    report("APT_RWY_END", &ends, (a.0.max(b.0), a.1.max(b.1)));
    let ord_ends: Vec<_> = ends
        .records
        .iter()
        .filter(|x| x.site_no == ord.site_no)
        .collect();
    assert!(ord_ends.len() >= 12);
    assert!(ord_ends
        .iter()
        .all(|e| ord_rwys.iter().any(|r| r.rwy_id == e.rwy_id)));
    assert!(ord_ends
        .iter()
        .any(|e| e.rwy_end_id == "10L" && e.position.is_some()));
}

// REQ: OA-NASR-004
#[test]
fn real_airways() {
    let Some(dir) = data_dir() else { return };
    let (bn, br) = file(&dir, "AWY_BASE");
    let (sn, sr) = file(&dir, "AWY_SEG_ALT");
    let aw = awy::parse_airways(&bn, br, &sn, sr).unwrap_or_else(|e| panic!("{e}"));
    report("AWY", &aw, (0.0, 0.0));
    assert!(aw.records.len() > 1_000);
    let j80 = aw
        .records
        .iter()
        .find(|a| a.base.awy_id == "J80")
        .expect("J80");
    assert!(j80.segments.len() > 10);
    assert!(j80
        .segments
        .windows(2)
        .all(|w| w[0].point_seq < w[1].point_seq));
    // Segments chain: each segment's TO_POINT is the next segment's FROM_POINT unless a gap.
    let mut breaks = 0;
    for w in j80.segments.windows(2) {
        if w[0].to_point.as_deref() != Some(w[1].from_point.as_str()) {
            breaks += 1;
            assert!(
                w[0].awy_seg_gap_flag,
                "unflagged break in J80 at line {}",
                w[0].line
            );
        }
    }
    println!(
        "J80: {} segments, {breaks} flagged gaps",
        j80.segments.len()
    );
    assert!(j80.segments.iter().any(|s| s.min_enroute_alt.alt.is_some()));
}

// REQ: OA-NASR-005
#[test]
fn real_stars_and_dps() {
    let Some(dir) = data_dir() else { return };
    for (kind, prefix) in [
        (procedure::ProcedureKind::Star, "STAR"),
        (procedure::ProcedureKind::Dp, "DP"),
    ] {
        let (bn, br) = file(&dir, &format!("{prefix}_BASE"));
        let (an, ar) = file(&dir, &format!("{prefix}_APT"));
        let (rn, rr) = file(&dir, &format!("{prefix}_RTE"));
        let p = procedure::parse_procedures(kind, (&bn, br), (&an, ar), (&rn, rr))
            .unwrap_or_else(|e| panic!("{e}"));
        report(prefix, &p, (0.0, 0.0));
        let no_routes = p.records.iter().filter(|x| x.routes.is_empty()).count();
        let no_airports = p.records.iter().filter(|x| x.airports.is_empty()).count();
        println!("{prefix}: {no_routes} procedures without routes, {no_airports} without airports");
        let complete = p
            .records
            .iter()
            .filter(|x| !x.routes.is_empty() && !x.airports.is_empty());
        assert!(complete.count() > 400);
        // The ZAU-owned procedures include the ORD ones.
        assert!(p
            .records
            .iter()
            .any(|x| x.base.artcc.as_deref() == Some("ZAU")
                && procedure::served_airports(&x.base).any(|a| a == "ORD")));
        assert!(p.records.iter().any(|x| x
            .routes
            .iter()
            .any(|r| r.portion == procedure::RoutePortion::Transition)));
    }
}

// REQ: OA-NASR-006
#[test]
fn real_class_airspace() {
    let Some(dir) = data_dir() else { return };
    let (n, r) = file(&dir, "CLS_ARSP");
    let c = cls::parse_class_airspace(&n, r).unwrap_or_else(|e| panic!("{e}"));
    report("CLS_ARSP", &c, (0.0, 0.0));
    assert!(c.rejected.is_empty());
    let ord = c
        .records
        .iter()
        .find(|x| x.arpt_id == "ORD")
        .expect("ORD class airspace");
    assert!(ord.class_b && !ord.class_c);
    assert!(c.records.iter().filter(|x| x.class_b).count() >= 30);
}
