//! An airport is its identifier plus its ICAO region: the lookups built from a model keep the
//! airports of two regions that share an identifier apart. Every identifier is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_airspace::model::{empty_model, point_kind, Meta, Model, StrIx};
use oa_bench::airspace_env::lookup_from_model;

fn s_t(m: &mut Model, t: &str) -> StrIx {
    m.strings.text.push_str(t);
    m.strings.offsets.push(m.strings.text.len() as u32);
    StrIx::new((m.strings.offsets.len() - 2) as u32)
}

/// Airports `KXYZ` in the given regions (with an elevation each) and one runway threshold of
/// each, scoped by `KXYZ` and carrying its airport's region.
fn model(regions: &[(&str, Option<i32>)]) -> Model {
    let mut m = empty_model(Meta {
        nasr_effective: 20261029,
        cifp_cycle: 2611,
        artcc: "ZAA".to_string(),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    });
    let (apt, rw) = (s_t(&mut m, "KXYZ"), s_t(&mut m, "RW09"));
    for (k, &(region, elev)) in regions.iter().enumerate() {
        let r = s_t(&mut m, region);
        let lat = 41.0 + k as f64;
        // The airport, then its threshold 0.01 degrees east.
        for (kind, ident, scope, e, dlon) in [
            (point_kind::AIRPORT, apt, StrIx::NONE, elev, 0.0),
            (point_kind::THRESHOLD, rw, apt, None, 0.01),
        ] {
            m.points.kind.push(kind);
            m.points.lat.push((lat * 1e7_f64).round() as i32);
            m.points.lon.push(((-88.0 + dlon) * 1e7_f64).round() as i32);
            m.points.elev_ft.push(e.unwrap_or(i32::MIN));
            m.points.ident.push(ident);
            m.points.alt_ident.push(StrIx::NONE);
            m.points.region.push(r);
            m.points.scope.push(scope);
            m.points.name.push(StrIx::NONE);
        }
    }
    m
}

// REQ: OA-BENCH-128
#[test]
fn an_identifier_in_two_regions_is_not_answered_and_is_counted() {
    let l = lookup_from_model(&model(&[("K5", Some(600)), ("K6", Some(900))]));
    assert!(l.airport("KXYZ").is_none());
    assert_eq!(l.ambiguous_airports, 1);
    // Both airports are kept, each with its own elevation and threshold.
    let (a, b) = (
        l.airport_in("KXYZ", "K5").unwrap(),
        l.airport_in("KXYZ", "K6").unwrap(),
    );
    assert_eq!((a.elev_ft, b.elev_ft), (600, 900));
    assert_eq!(a.thresholds, vec![(41.0, -87.99)]);
    assert_eq!(b.thresholds, vec![(42.0, -87.99)]);
    assert!(l.airport_in("KXYZ", "K7").is_none());
}

// REQ: OA-BENCH-128
#[test]
fn thresholds_join_the_airport_of_their_own_region_only() {
    // The K6 airport has no published elevation, so it is not offered, and the K5 airport keeps
    // its own threshold (not the K6 one with the same scope).
    let l = lookup_from_model(&model(&[("K5", Some(600)), ("K6", None)]));
    let a = l.airport("KXYZ").expect("the K5 airport");
    assert_eq!(a.elev_ft, 600);
    assert_eq!(a.thresholds, vec![(41.0, -87.99)]);
    assert_eq!(l.ambiguous_airports, 0);
    // The other way round: the thresholds are the K6 airport's.
    let l = lookup_from_model(&model(&[("K5", None), ("K6", Some(900))]));
    let a = l.airport("KXYZ").unwrap();
    assert_eq!((a.elev_ft, a.thresholds), (900, vec![(42.0, -87.99)]));
}
