//! Small-angle kernels: the sine, cosine and arctangent of arguments near zero, from truncated
//! series in `+ - * /` only, so they are bit-identical on every machine without a library call.
//! The local frame works with angles of a few hundredths of a radian (a radar's range over the
//! Earth's radius), where these series are exact to the last bit of the result; larger arguments
//! go to [`oa_math`].

/// The largest `|a|` for which [`sin_cos_small`] is used (radians; about 1,600 km on the Earth).
pub const SIN_COS_LIMIT: f64 = 0.25;
/// The largest `|t|` for which [`atan_small`] is used.
pub const ATAN_LIMIT: f64 = 0.2;

/// `sin a = a (1 + a^2 SIN[0] + a^4 SIN[1] + ...)`: `(-1)^k / (2k + 1)!` from `k = 1`, through
/// `a^15`.
const SIN: [f64; 7] = [
    -1.0 / 6.0,
    1.0 / 120.0,
    -1.0 / 5_040.0,
    1.0 / 362_880.0,
    -1.0 / 39_916_800.0,
    1.0 / 6_227_020_800.0,
    -1.0 / 1_307_674_368_000.0,
];
/// `cos a = 1 + a^2 COS[0] + a^4 COS[1] + ...`: `(-1)^k / (2k)!` from `k = 1`, through `a^16`.
const COS: [f64; 8] = [
    -1.0 / 2.0,
    1.0 / 24.0,
    -1.0 / 720.0,
    1.0 / 40_320.0,
    -1.0 / 3_628_800.0,
    1.0 / 479_001_600.0,
    -1.0 / 87_178_291_200.0,
    1.0 / 20_922_789_888_000.0,
];
/// `atan t = t (1 + t^2 ATAN[0] + t^4 ATAN[1] + ...)`: `(-1)^k / (2k + 1)` from `k = 1`, through
/// `t^25`.
const ATAN: [f64; 12] = [
    -1.0 / 3.0,
    1.0 / 5.0,
    -1.0 / 7.0,
    1.0 / 9.0,
    -1.0 / 11.0,
    1.0 / 13.0,
    -1.0 / 15.0,
    1.0 / 17.0,
    -1.0 / 19.0,
    1.0 / 21.0,
    -1.0 / 23.0,
    1.0 / 25.0,
];

/// `1 + x c[0] + x^2 c[1] + ...`, highest term first (a fixed order).
fn horner(x: f64, c: &[f64]) -> f64 {
    1.0 + x * c.iter().rev().fold(0.0, |acc, &k| acc * x + k)
}

/// `(sin a, cos a)` for `|a| <= SIN_COS_LIMIT`: Taylor series whose first omitted terms are
/// below 2e-25 of the result.
pub fn sin_cos_small(a: f64) -> (f64, f64) {
    let a2 = a * a;
    (a * horner(a2, &SIN), horner(a2, &COS))
}

/// `(sin a, cos a)`: the series for a small `a`, [`oa_math::sin_cos`] otherwise (and for NaN).
pub fn sin_cos_near_zero(a: f64) -> (f64, f64) {
    if a.abs() <= SIN_COS_LIMIT {
        sin_cos_small(a)
    } else {
        oa_math::sin_cos(a)
    }
}

/// `atan t` for `|t| <= ATAN_LIMIT`: the alternating series, whose first omitted term is below
/// 3e-20 of the result.
pub fn atan_small(t: f64) -> f64 {
    t * horner(t * t, &ATAN)
}

/// The angle in `[0, pi]` whose sine and cosine are `s >= 0` and `c`: the half-angle series when
/// the angle is under 0.25 rad (the usual case, a point within 1,600 km), [`oa_math::atan2`]
/// otherwise.
pub fn angle_from_sin_cos(s: f64, c: f64) -> f64 {
    if c >= 0.968 && s >= 0.0 {
        // tan(a / 2) = s / (1 + c) <= tan(0.128).
        2.0 * atan_small(s / (1.0 + c))
    } else {
        oa_math::atan2(s, c)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // REQ: OA-GEO-015
    #[test]
    fn the_small_angle_series_agree_with_oa_math_to_a_couple_of_ulp() {
        let mut worst = 0.0f64;
        for k in -2_000..=2_000 {
            let a = SIN_COS_LIMIT * f64::from(k) / 2_000.0;
            let (s, c) = sin_cos_small(a);
            let (rs, rc) = oa_math::sin_cos(a);
            assert!((c - rc).abs() <= 3.0e-16, "cos {a}: {c} vs {rc}");
            assert!(
                (s - rs).abs() <= 3.0e-16 * rs.abs().max(1.0e-300) + 1.0e-300,
                "sin {a}"
            );
            worst = worst.max((s - rs).abs() / rs.abs().max(1.0e-300));
            let t = ATAN_LIMIT * f64::from(k) / 2_000.0;
            let (g, rg) = (atan_small(t), oa_math::atan2(t, 1.0));
            assert!(
                (g - rg).abs() <= 3.0e-16 * rg.abs().max(1.0e-300) + 1.0e-300,
                "atan {t}"
            );
        }
        assert!(worst < 3.0e-16, "{worst}");
        // Outside the limit the library functions answer.
        assert_eq!(sin_cos_near_zero(1.0), oa_math::sin_cos(1.0));
        assert_eq!(sin_cos_near_zero(-3.0), oa_math::sin_cos(-3.0));
        assert!(sin_cos_near_zero(f64::NAN).0.is_nan());
    }

    // REQ: OA-GEO-015
    #[test]
    fn an_angle_from_its_sine_and_cosine_is_the_arctangent_in_every_range() {
        for k in 0..=3_100 {
            let a = f64::from(k) * 0.001;
            let (s, c) = (a.sin(), a.cos());
            let got = angle_from_sin_cos(s, c);
            assert!((got - a).abs() <= 4.0e-16 * a.max(1.0e-3), "{a}: {got}");
        }
        assert!(angle_from_sin_cos(f64::NAN, 1.0).is_nan());
    }
}
