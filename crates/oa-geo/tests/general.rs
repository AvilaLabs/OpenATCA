//! Crate-wide rules: no unsafe, no dependencies, and no panics on arbitrary input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::{
    dms_char_to_decimal, dms_to_decimal, hemisphere_from_char, wgs84, Hemisphere, LatLon, Polygon,
};

// REQ: OA-GEN-001
#[test]
fn every_source_file_is_unsafe_free_and_lib_forbids_unsafe() {
    let lib = include_str!("../src/lib.rs");
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    for (name, text) in [
        ("lib.rs", lib),
        ("error.rs", include_str!("../src/error.rs")),
        ("geodesic.rs", include_str!("../src/geodesic.rs")),
        ("latlon.rs", include_str!("../src/latlon.rs")),
        ("polygon.rs", include_str!("../src/polygon.rs")),
        ("rhumb.rs", include_str!("../src/rhumb.rs")),
        ("coef.rs", include_str!("../src/coef.rs")),
    ] {
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let has_unsafe = code
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == "unsafe");
            let is_forbid = code.contains("forbid(unsafe_code)");
            assert!(!has_unsafe || is_forbid, "{name}:{}: unsafe code", n + 1);
        }
    }
}

// REQ: OA-GEN-002
#[test]
fn manifest_declares_no_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let mut section = String::new();
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            assert!(
                !t.contains("dev-dependencies") && !t.contains("build-dependencies"),
                "unexpected section {t}"
            );
            continue;
        }
        if section == "[dependencies]" {
            // The workspace crates allowed: the elementary functions and the IEEE-754
            // operations `core` lacks (RULESET F1).
            assert!(
                t.is_empty()
                    || t.starts_with('#')
                    || t == "oa-math = { path = \"../oa-math\" }"
                    || t == "oa-ieee = { path = \"../oa-ieee\" }"
                    || t == "oa-math = { path = \"../openatca-math\", package = \"openatca-math\", version = \"0.1.0\" }"
                    || t == "oa-ieee = { path = \"../openatca-ieee\", package = \"openatca-ieee\", version = \"0.1.0\" }",
                "dependency declared: {t}"
            );
        }
        if section.starts_with("[dependencies.") || section.starts_with("[target.") {
            panic!("dependency table {section}");
        }
    }
}

/// Deterministic stream of awkward f64 values: raw random bit patterns (including NaN and the
/// infinities), extremes, and ordinary values.
struct Awkward(u64);

impl Awkward {
    fn next_bits(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
    fn value(&mut self) -> f64 {
        const SPECIAL: [f64; 14] = [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            f64::EPSILON,
            0.0,
            -0.0,
            90.0,
            -90.0,
            180.0,
            -180.0,
            1e300,
        ];
        let r = self.next_bits();
        match (r >> 40) % 4 {
            0 => SPECIAL[(r >> 20) as usize % SPECIAL.len()],
            1 => f64::from_bits(self.next_bits()),
            2 => ((self.next_bits() >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 800.0,
            _ => ((self.next_bits() >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 1e8,
        }
    }
    fn valid_point(&mut self) -> LatLon {
        let lat = ((self.next_bits() >> 11) as f64 / (1u64 << 53) as f64) * 180.0 - 90.0;
        let lon = ((self.next_bits() >> 11) as f64 / (1u64 << 53) as f64) * 360.0 - 180.0;
        LatLon::new(lat, lon).expect("valid")
    }
}

// REQ: OA-GEN-004
#[test]
fn arbitrary_numbers_never_panic_and_non_finite_is_rejected() {
    let e = wgs84();
    let mut g = Awkward(2024);
    let mut accepted = 0;
    for _ in 0..4000 {
        let (a, b, c, d) = (g.value(), g.value(), g.value(), g.value());
        if let Ok(p) = LatLon::new(a, b) {
            accepted += 1;
            assert!(a.is_finite() && b.is_finite());
            let q = g.valid_point();
            if let Ok(r) = e.inverse(p, q) {
                assert!(r.distance_m.is_finite());
            }
            for x in [
                e.direct(p, c, d).map(|x| x.position),
                e.rhumb_direct(p, c, d),
            ]
            .into_iter()
            .flatten()
            {
                assert!(x.lat().is_finite() && x.lon().is_finite());
            }
            let _ = e.rhumb_inverse(p, q);
            let _ = e.rhumb_point_at_fraction(p, q, c);
        } else {
            assert!(
                !(a.is_finite()
                    && b.is_finite()
                    && (-90.0..=90.0).contains(&a)
                    && (-180.0..=180.0).contains(&b))
            );
        }
        let _ = dms_to_decimal(a, b, c, Hemisphere::North);
        let _ = dms_char_to_decimal(a, b, c, 'W');
        let _ = hemisphere_from_char(if a > 0.0 { 'E' } else { 'x' });
        let _ = e.direct(g.valid_point(), c, d);
    }
    assert!(accepted > 300, "only {accepted} finite in-range pairs");
    // The separation search is expensive, so it gets fewer random trials.
    for _ in 0..12 {
        let _ = e.geodesic_rhumb_separation(g.valid_point(), g.valid_point(), 2);
    }
}

// REQ: OA-GEN-004
#[test]
fn random_valid_points_and_polygons_never_panic() {
    let e = wgs84();
    let mut g = Awkward(99);
    for _ in 0..3000 {
        let p = g.valid_point();
        let q = g.valid_point();
        let r = e.inverse(p, q).expect("every valid pair converges");
        assert!(r.distance_m >= 0.0 && r.distance_m <= 20_037_509.0);
    }
    for _ in 0..300 {
        let n = 3 + (g.next_bits() % 6) as usize;
        let ring: Vec<LatLon> = (0..n).map(|_| g.valid_point()).collect();
        if let Ok(poly) = Polygon::new(&e, &ring) {
            let _ = poly.contains(g.valid_point());
        }
    }
    // Extreme but legal locations.
    let extremes = [
        (90.0, 180.0),
        (-90.0, -180.0),
        (90.0, -180.0),
        (0.0, 180.0),
        (0.0, -180.0),
        (-0.0, 0.0),
    ];
    for &(la, lo) in &extremes {
        for &(lb, lob) in &extremes {
            let a = LatLon::new(la, lo).expect("valid");
            let b = LatLon::new(lb, lob).expect("valid");
            assert!(e.inverse(a, b).is_ok(), "{la} {lo} -> {lb} {lob}");
            let _ = e.rhumb_inverse(a, b);
        }
    }
}

// REQ: OA-GEN-002
#[test]
fn library_is_no_std_and_uses_no_std_only_float_methods() {
    // With oa-ieee in the dependency graph the compiler still resolves `f64::sqrt` and the
    // other std-only inherent methods, so this scan is what keeps them out.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = std::fs::read_to_string(dir.join("lib.rs")).unwrap();
    assert!(lib.contains("#![cfg_attr(not(test), no_std)]"));
    let banned = [
        "sqrt", "round", "floor", "ceil", "trunc", "hypot", "powi", "powf", "mul_add",
    ];
    let mut hits = Vec::new();
    let mut scanned = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        // Everything from the first `#[cfg(test)]` on is test code, which has std.
        let body = text.split("#[cfg(test)]").next().unwrap();
        scanned += 1;
        for (n, line) in body.lines().enumerate() {
            let code = line.split("//").next().unwrap();
            for b in banned {
                if code.contains(&format!(".{b}(")) || code.contains(&format!("f64::{b}")) {
                    hits.push(format!("{}:{}: {b}", path.display(), n + 1));
                }
            }
        }
    }
    assert!(scanned >= 8);
    assert!(hits.is_empty(), "{hits:#?}");
}
