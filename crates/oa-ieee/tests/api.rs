//! The public API is pinned, and each function is checked on the special values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[test]
fn public_functions_are_exactly_the_documented_list() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    let mut names: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("pub fn "))
        .filter_map(|l| l.split('(').next())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["ceil", "floor", "round", "sqrt", "trunc"]);
    // Nothing else public: no other `pub` item, `use`, module or macro.
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with("pub ") {
            assert!(t.starts_with("pub fn "), "unexpected public item: {l}");
        }
        assert!(
            !t.starts_with("mod ") && !t.starts_with("macro_rules!"),
            "{l}"
        );
    }
}

fn specials() -> Vec<f64> {
    let mut v = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        1.5,
        -1.5,
        2.5,
        -2.5,
        0.49999999999999994,
        -0.49999999999999994,
        4503599627370495.5,
        4503599627370496.0,
        9007199254740993.0,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::from_bits(0x800f_ffff_ffff_ffff),
        f64::EPSILON,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -f64::NAN,
    ];
    for k in 0..64 {
        v.push(f64::from_bits(1u64 << k));
    }
    v
}

fn same(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}

#[test]
fn each_function_equals_std_on_special_values() {
    for x in specials() {
        assert!(same(oa_ieee::sqrt(x), x.sqrt()), "sqrt {x:e}");
        assert!(same(oa_ieee::floor(x), x.floor()), "floor {x:e}");
        assert!(same(oa_ieee::ceil(x), x.ceil()), "ceil {x:e}");
        assert!(same(oa_ieee::trunc(x), x.trunc()), "trunc {x:e}");
        assert!(same(oa_ieee::round(x), x.round()), "round {x:e}");
    }
}

#[test]
fn documented_behaviour() {
    assert_eq!(oa_ieee::sqrt(-0.0).to_bits(), (-0.0f64).to_bits());
    assert!(oa_ieee::sqrt(-1.0).is_nan());
    assert!(oa_ieee::sqrt(f64::NEG_INFINITY).is_nan());
    assert_eq!(oa_ieee::sqrt(f64::INFINITY), f64::INFINITY);
    assert_eq!(oa_ieee::sqrt(4.0), 2.0);
    assert_eq!(oa_ieee::round(0.5), 1.0);
    assert_eq!(oa_ieee::round(-0.5), -1.0);
    assert_eq!(oa_ieee::round(2.5), 3.0);
    assert_eq!(oa_ieee::round(0.49999999999999994), 0.0);
    assert_eq!(oa_ieee::round(-0.2).to_bits(), (-0.0f64).to_bits());
    assert_eq!(oa_ieee::floor(-0.5), -1.0);
    assert_eq!(oa_ieee::ceil(-0.5).to_bits(), (-0.0f64).to_bits());
    assert_eq!(oa_ieee::trunc(-1.9), -1.0);
    for f in [
        oa_ieee::floor,
        oa_ieee::ceil,
        oa_ieee::trunc,
        oa_ieee::round,
    ] {
        assert_eq!(f(f64::INFINITY), f64::INFINITY);
        assert_eq!(f(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert!(f(f64::NAN).is_nan());
        assert_eq!(f(-0.0).to_bits(), (-0.0f64).to_bits());
    }
}
