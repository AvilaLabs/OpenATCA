//! Accuracy against correctly rounded mpmath references (tools/oa-math-ref/gen_ref.py).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// One test case: inputs, the correctly rounded result and the remainder true - hi.
struct Case {
    x: f64,
    y: f64,
    hi: f64,
    lo: f64,
}

/// The committed data, or the file of the same name in `OA_MATH_REF_DIR` for a larger run.
fn data(name: &str, committed: &'static [u8]) -> Vec<u8> {
    match std::env::var("OA_MATH_REF_DIR") {
        Ok(dir) => std::fs::read(format!("{dir}/{name}.bin")).unwrap(),
        Err(_) => committed.to_vec(),
    }
}

fn load(bytes: &[u8], arity: usize) -> Vec<Case> {
    let words: Vec<u64> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect();
    let rec = arity + 2;
    assert_eq!(words.len() % rec, 0);
    words
        .chunks_exact(rec)
        .map(|w| Case {
            x: f64::from_bits(w[0]),
            y: if arity == 2 {
                f64::from_bits(w[1])
            } else {
                0.0
            },
            hi: f64::from_bits(w[arity]),
            lo: f64::from_bits(w[arity + 1]),
        })
        .collect()
}

/// Spacing of the doubles at the true value hi + lo.
fn ulp_at(hi: f64, lo: f64) -> f64 {
    let a = hi.abs();
    let bits = a.to_bits();
    let e = (bits >> 52) as i32;
    if e == 0 {
        return f64::from_bits(1); // 2^-1074
    }
    let mut ulp_exp = e - 1023 - 52;
    // true value just below a power of two: the spacing is half
    if bits & 0x000f_ffff_ffff_ffff == 0 && (lo < 0.0) == (hi > 0.0) && lo != 0.0 && e > 1 {
        ulp_exp -= 1;
    }
    if ulp_exp < -1022 {
        f64::from_bits(1u64 << (ulp_exp + 1074))
    } else {
        f64::from_bits(((ulp_exp + 1023) as u64) << 52)
    }
}

/// Error of `c` in ulps of the true value; infinite for a wrong NaN, zero or infinity.
fn err_ulps(c: f64, case: &Case) -> f64 {
    if case.hi.is_infinite() || case.hi == 0.0 {
        return if c.to_bits() == case.hi.to_bits() {
            0.0
        } else {
            f64::INFINITY
        };
    }
    if !c.is_finite() {
        return f64::INFINITY;
    }
    ((c - case.hi) - case.lo).abs() / ulp_at(case.hi, case.lo)
}

fn check1(name: &str, committed: &'static [u8], f: impl Fn(f64) -> f64) -> f64 {
    let cases = load(&data(name, committed), 1);
    assert!(cases.len() >= 3000, "{name}: {} cases", cases.len());
    let mut worst = 0.0f64;
    let mut at = 0.0;
    let mut not_correct = 0usize;
    for c in &cases {
        let r = f(c.x);
        let e = err_ulps(r, c);
        if r.to_bits() != c.hi.to_bits() {
            not_correct += 1;
        }
        if e > worst {
            worst = e;
            at = c.x;
        }
    }
    println!(
        "{name:6} cases {:5}  max error {worst:.6} ulp at x = {at:e}  not correctly rounded {not_correct}",
        cases.len()
    );
    assert!(worst < 1.0, "{name}: max error {worst} ulp at {at:e}");
    worst
}

fn check2(name: &str, committed: &'static [u8], f: impl Fn(f64, f64) -> f64) -> f64 {
    let cases = load(&data(name, committed), 2);
    assert!(cases.len() >= 3000, "{name}: {} cases", cases.len());
    let mut worst = 0.0f64;
    let mut at = (0.0, 0.0);
    let mut not_correct = 0usize;
    for c in &cases {
        let r = f(c.x, c.y);
        let e = err_ulps(r, c);
        if r.to_bits() != c.hi.to_bits() {
            not_correct += 1;
        }
        if e > worst {
            worst = e;
            at = (c.x, c.y);
        }
    }
    println!(
        "{name:6} cases {:5}  max error {worst:.6} ulp at (x, y) = ({:e}, {:e})  not correctly rounded {not_correct}",
        cases.len(),
        at.0,
        at.1
    );
    assert!(worst < 1.0, "{name}: max error {worst} ulp at {at:?}");
    worst
}

// REQ: OA-MATH-001
#[test]
fn sin_cos_tan_against_reference() {
    check1("sin", include_bytes!("data/sin.bin"), oa_math::sin);
    check1("cos", include_bytes!("data/cos.bin"), oa_math::cos);
    check1("tan", include_bytes!("data/tan.bin"), oa_math::tan);
}

// REQ: OA-MATH-002
#[test]
fn inverse_trig_against_reference() {
    check1("atan", include_bytes!("data/atan.bin"), oa_math::atan);
    check1("asin", include_bytes!("data/asin.bin"), oa_math::asin);
    check1("acos", include_bytes!("data/acos.bin"), oa_math::acos);
    check2("atan2", include_bytes!("data/atan2.bin"), oa_math::atan2);
}

// REQ: OA-MATH-003
#[test]
fn exp_ln_against_reference() {
    check1("exp", include_bytes!("data/exp.bin"), oa_math::exp);
    check1("ln", include_bytes!("data/ln.bin"), oa_math::ln);
}

// REQ: OA-MATH-004
#[test]
fn pow_against_reference() {
    check2("pow", include_bytes!("data/pow.bin"), oa_math::pow);
}

// REQ: OA-MATH-005
#[test]
fn hypot_cbrt_against_reference() {
    check2("hypot", include_bytes!("data/hypot.bin"), oa_math::hypot);
    check1("cbrt", include_bytes!("data/cbrt.bin"), oa_math::cbrt);
}
