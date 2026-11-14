//! Whole-file test against the real FAA CIFP. Needs `OPENATCA_CIFP` (path to `FAACIFP18`);
//! skips with a message when it is unset.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

use oa_arinc424::{
    controlling_agency, cycle_is_after, find_airport, is_clean, is_closed, mora_at, parse_reader,
    procedures_at, runway_designator, runways_at, AirspaceKind, BoundaryVia, Cifp, Cycle,
    PathTerminator, ProcedureKind,
};

fn load() -> Option<(Cifp, std::time::Duration)> {
    let path = std::env::var("OPENATCA_CIFP")
        .ok()
        .filter(|p| !p.is_empty());
    let Some(path) = path else {
        eprintln!("OPENATCA_CIFP is not set: skipping the full-file CIFP test");
        return None;
    };
    let f = File::open(&path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let t = Instant::now();
    let cifp =
        parse_reader(BufReader::with_capacity(1 << 16, f), &path).unwrap_or_else(|e| panic!("{e}"));
    Some((cifp, t.elapsed()))
}

// REQ: OA-424-001, OA-424-002, OA-424-003, OA-424-004, OA-424-005, OA-424-006, OA-GEN-003, OA-GEN-004
#[test]
fn the_real_cifp_parses_with_zero_errors_and_expected_content() {
    let Some((c, took)) = load() else { return };
    eprintln!(
        "parsed {} records in {took:?}",
        c.counts.values().sum::<usize>()
    );
    eprintln!("counts per section: {:?}", c.counts);

    // Zero record errors for every in-scope type (any error is listed).
    assert!(
        is_clean(&c),
        "{} errors, first: {:?}",
        c.errors.len() + c.errors_dropped,
        c.errors.iter().take(10).collect::<Vec<_>>()
    );

    // Header and cycle.
    let h = c.header.as_ref().expect("header");
    assert_eq!(
        h.cycle,
        Cycle {
            year: 26,
            release: 11
        }
    );
    assert_eq!(h.record_count, 397_304);
    assert_eq!(c.counts.values().sum::<usize>(), 397_304);
    assert!(c.cycle_counts.keys().all(|k| !cycle_is_after(*k, h.cycle)));
    eprintln!(
        "record cycles: {} distinct, oldest {:?}",
        c.cycle_counts.len(),
        c.cycle_counts.keys().next()
    );

    // Record counts per type (physical records, continuations included), from the file.
    let want: BTreeMap<&str, usize> = [
        ("AS", 241),
        ("D", 2085),
        ("DB", 373),
        ("EA", 32_547),
        ("ER", 19_392),
        ("PA", 13_319),
        ("PC", 37_509),
        ("PD", 34_219),
        ("PN", 181),
        ("PE", 44_962),
        ("PF", 128_419),
        ("PG", 16_990),
        ("PI", 1278),
        ("UC", 13_135),
        ("UR", 30_536),
    ]
    .into_iter()
    .collect();
    for (k, v) in &want {
        assert_eq!(c.counts.get(*k), Some(v), "section {k}");
    }
    // Decoded objects.
    assert_eq!(c.airports.len(), 13_319);
    assert_eq!(c.runways.len(), 16_990);
    assert_eq!(c.localizers.len(), 1278);
    assert_eq!(c.vhf_navaids.len(), 2085);
    assert_eq!(c.ndbs.len(), 373);
    assert_eq!(c.terminal_ndbs.len(), 181);
    assert_eq!(c.enroute_waypoints.len(), 32_547);
    assert_eq!(c.terminal_waypoints.len(), 37_509);
    assert_eq!(c.mora.len(), 241);
    let fixes: usize = c.airways.iter().map(|a| a.fixes.len()).sum();
    assert_eq!(fixes, 19_392);
    let legs: usize = c
        .procedures
        .iter()
        .flat_map(|p| &p.routes)
        .map(|r| r.legs.len())
        .sum();
    let conts: usize = c
        .procedures
        .iter()
        .flat_map(|p| &p.routes)
        .flat_map(|r| &r.legs)
        .map(|l| l.continuations.len())
        .sum();
    assert_eq!(legs + conts, 34_219 + 44_962 + 128_419);
    assert_eq!(conts, 6747); // level-of-service continuation records on approach legs
    let points: usize = c.controlled_airspace.iter().map(|a| a.boundary.len()).sum();
    assert_eq!(points, 13_135);
    let ur_points: usize = c
        .special_use_airspace
        .iter()
        .map(|a| a.boundary.len())
        .sum();
    let ur_conts: usize = c
        .special_use_airspace
        .iter()
        .map(|a| a.continuations.len())
        .sum();
    assert_eq!(ur_points + ur_conts, 30_536);

    // KORD exists with its runways.
    let ord = find_airport(&c, "KORD").expect("KORD");
    assert_eq!(ord.name, "CHICAGO O'HARE INTL");
    let mut rw: Vec<&str> = runways_at(&c, "KORD").map(runway_designator).collect();
    rw.sort_unstable();
    assert_eq!(
        rw,
        [
            "04L", "04R", "09C", "09L", "09R", "10C", "10L", "10R", "22L", "22R", "27C", "27L",
            "27R", "28C", "28L", "28R"
        ]
    );

    // Procedures: KORD has STARs and approaches. It has no SIDs in this cycle: neither the
    // CIFP nor NASR DP_APT lists a departure procedure for ORD. SIDs exist elsewhere.
    let stars = procedures_at(&c, "KORD", ProcedureKind::Star).count();
    assert!(stars >= 1, "KORD STARs");
    assert!(procedures_at(&c, "KORD", ProcedureKind::Approach).count() >= 1);
    eprintln!(
        "KORD: {} STARs, {} SIDs, {} approaches",
        stars,
        procedures_at(&c, "KORD", ProcedureKind::Sid).count(),
        procedures_at(&c, "KORD", ProcedureKind::Approach).count()
    );
    let sid_airports: std::collections::BTreeSet<&str> = c
        .procedures
        .iter()
        .filter(|p| p.kind == ProcedureKind::Sid)
        .map(|p| p.airport_ident.as_str())
        .collect();
    assert!(
        sid_airports.len() > 100,
        "SIDs at {} airports",
        sid_airports.len()
    );
    assert!(procedures_at(&c, "KMSP", ProcedureKind::Sid).count() >= 1);
    // Every leg of every procedure is in strictly increasing sequence and uses a known
    // path terminator (decoding fails otherwise); an IF/TF-led STAR has a first leg.
    for p in &c.procedures {
        for r in &p.routes {
            assert!(!r.legs.is_empty());
            assert!(r.legs.windows(2).all(|w| w[0].sequence < w[1].sequence));
        }
    }
    let ord_star = procedures_at(&c, "KORD", ProcedureKind::Star)
        .next()
        .expect("a STAR");
    assert!(ord_star.routes.iter().any(|r| r
        .legs
        .iter()
        .any(|l| l.path_terminator == PathTerminator::TF)));

    // Airspace: the ORD Class B is a set of closed boundaries; every boundary in the file is closed.
    let ord_b: Vec<_> = c
        .controlled_airspace
        .iter()
        .filter(|a| a.designation == "KORD" && a.classification == Some('B'))
        .collect();
    assert!(ord_b.len() >= 8, "ORD Class B areas: {}", ord_b.len());
    for a in &ord_b {
        assert_eq!(a.kind, AirspaceKind::Controlled);
        assert!(is_closed(a), "{} area {}", a.name, a.multiple_code);
        assert!(a.boundary.len() >= 2);
        assert!(a.boundary.iter().any(
            |p| p.via == BoundaryVia::ClockwiseArc || p.via == BoundaryVia::CounterClockwiseArc
        ));
        assert!(a.lower.is_some() && a.upper.is_some());
    }
    let open: Vec<_> = c
        .controlled_airspace
        .iter()
        .chain(&c.special_use_airspace)
        .filter(|a| !is_closed(a))
        .map(|a| (a.meta.line, a.designation.clone()))
        .collect();
    assert!(
        open.is_empty(),
        "{} unclosed boundaries, e.g. {:?}",
        open.len(),
        open.iter().take(5).collect::<Vec<_>>()
    );
    assert!(
        c.special_use_airspace
            .iter()
            .filter(|a| controlling_agency(a).is_some())
            .count()
            >= 1000
    );

    // Grid MORA covers 41 N 88 W.
    let m = mora_at(&c, 41.0, -88.0).expect("MORA at 41N 88W");
    assert_eq!(m, 3500);
    assert_eq!(mora_at(&c, 36.578, -118.292), Some(16_800)); // Mount Whitney cell

    // Internal consistency (OA-424-007): a runway end whose crossing-height indicator is `I`
    // names a localizer, and that localizer's glide slope crossing height is the same number.
    let (mut checked, mut same) = (0usize, 0usize);
    for r in c.runways.iter().filter(|r| r.tch_indicator == Some('I')) {
        let Some(loc) = r.localizer_ident.as_deref() else {
            continue;
        };
        let l = c.localizers.iter().find(|l| {
            l.airport_ident == r.airport_ident
                && l.airport_region == r.airport_region
                && l.ident == loc
                && l.runway_ident == r.ident
        });
        if let Some(l) = l {
            checked += 1;
            same += usize::from(l.glide_slope_tch_ft == r.threshold_crossing_height_ft);
        }
    }
    eprintln!("runway TCH (indicator I) equals the localizer glide slope TCH on {same} of {checked} runway ends");
    assert!(checked > 500 && same * 100 >= checked * 95);

    // Boundary geometry (OA-424-005, OA-424-007): for arcs the coded distance and bearing
    // are those from the arc origin to the point; check them against the coordinates with an
    // independent spherical computation. The FAA readme says some arc ends were recalculated.
    let (mut arcs, mut dist_ok, mut brg_ok) = (0usize, 0usize, 0usize);
    let mut worst = 0.0f64;
    let mut ends_missing = 0usize;
    for a in c.controlled_airspace.iter().chain(&c.special_use_airspace) {
        if !a.boundary.last().is_some_and(|p| p.end) {
            ends_missing += 1;
        }
        for p in &a.boundary {
            if let (Some(pt), Some(o), Some(d), Some(b)) = (
                p.position,
                p.arc_origin,
                p.arc_distance_nm,
                p.arc_bearing_deg,
            ) {
                arcs += 1;
                let (dd, bb) = great_circle(o.lat, o.lon, pt.lat, pt.lon);
                let tol = 0.03 + 0.005 * d;
                worst = worst.max((dd - d).abs() / d.max(0.1));
                dist_ok += usize::from((dd - d).abs() <= tol);
                let db = ((bb - b + 540.0) % 360.0 - 180.0).abs();
                brg_ok += usize::from(db <= 0.2 + 30.0 / (d.max(0.1) * 60.0));
            }
        }
    }
    eprintln!("arc geometry: {arcs} arc points, distance matches {dist_ok}, bearing matches {brg_ok}, worst relative distance error {worst:.4}, volumes without an end marker {ends_missing}");
    assert!(arcs > 2000);
    assert!(dist_ok * 100 >= arcs * 98 && brg_ok * 100 >= arcs * 98);
    assert_eq!(ends_missing, 0);
}

/// Distance in NM and initial bearing from point 1 to point 2 on a sphere of radius 3440.065 NM.
fn great_circle(la1: f64, lo1: f64, la2: f64, lo2: f64) -> (f64, f64) {
    let (la1, lo1, la2, lo2) = (
        la1.to_radians(),
        lo1.to_radians(),
        la2.to_radians(),
        lo2.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    let d = 2.0 * h.sqrt().asin() * 3440.065;
    let y = (lo2 - lo1).sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * (lo2 - lo1).cos();
    (d, y.atan2(x).to_degrees().rem_euclid(360.0))
}
