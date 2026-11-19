//! 3 x 3 linear algebra for the smoother. Core (pure, B7). Plain arrays and destructuring, no
//! indexing, so no operation can panic (E3). Every sum is written out in a fixed order, which
//! is part of the determinism guarantee (see the crate documentation).

/// A 3-vector.
pub(crate) type V3 = [f64; 3];
/// A 3 x 3 matrix, row major.
pub(crate) type M3 = [V3; 3];
/// A symmetric 3 x 3 matrix packed as `[p00, p01, p02, p11, p12, p22]`.
pub(crate) type P6 = [f64; 6];

pub(crate) fn dot(a: &V3, b: &V3) -> f64 {
    let [a0, a1, a2] = *a;
    let [b0, b1, b2] = *b;
    a0 * b0 + a1 * b1 + a2 * b2
}

pub(crate) fn transpose(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    [[a, d, g], [b, e, h], [c, f, i]]
}

pub(crate) fn mul(a: &M3, b: &M3) -> M3 {
    let bt = transpose(b);
    a.map(|r| bt.map(|c| dot(&r, &c)))
}

pub(crate) fn matvec(m: &M3, v: &V3) -> V3 {
    m.map(|r| dot(&r, v))
}

fn zip_v(a: &V3, b: &V3, f: impl Fn(f64, f64) -> f64) -> V3 {
    let [a0, a1, a2] = *a;
    let [b0, b1, b2] = *b;
    [f(a0, b0), f(a1, b1), f(a2, b2)]
}

pub(crate) fn add_v(a: &V3, b: &V3) -> V3 {
    zip_v(a, b, |x, y| x + y)
}

pub(crate) fn sub_v(a: &V3, b: &V3) -> V3 {
    zip_v(a, b, |x, y| x - y)
}

pub(crate) fn add(a: &M3, b: &M3) -> M3 {
    let [a0, a1, a2] = *a;
    let [b0, b1, b2] = *b;
    [add_v(&a0, &b0), add_v(&a1, &b1), add_v(&a2, &b2)]
}

pub(crate) fn sub(a: &M3, b: &M3) -> M3 {
    let [a0, a1, a2] = *a;
    let [b0, b1, b2] = *b;
    [sub_v(&a0, &b0), sub_v(&a1, &b1), sub_v(&a2, &b2)]
}

/// The inverse by cofactors, or `None` if the determinant is zero or not finite.
pub(crate) fn inv(m: &M3) -> Option<M3> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let c00 = e * i - f * h;
    let c01 = f * g - d * i;
    let c02 = d * h - e * g;
    let det = a * c00 + b * c01 + c * c02;
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    let r = 1.0 / det;
    Some([
        [c00 * r, (c * h - b * i) * r, (b * f - c * e) * r],
        [c01 * r, (a * i - c * g) * r, (c * d - a * f) * r],
        [c02 * r, (b * g - a * h) * r, (a * e - b * d) * r],
    ])
}

pub(crate) fn unpack(p: &P6) -> M3 {
    let [p00, p01, p02, p11, p12, p22] = *p;
    [[p00, p01, p02], [p01, p11, p12], [p02, p12, p22]]
}

/// Packs a matrix, averaging the off-diagonal pairs so rounding cannot make it asymmetric.
pub(crate) fn pack(m: &M3) -> P6 {
    let [[p00, p01, p02], [p10, p11, p12], [p20, p21, p22]] = *m;
    [
        p00,
        (p01 + p10) / 2.0,
        (p02 + p20) / 2.0,
        p11,
        (p12 + p21) / 2.0,
        p22,
    ]
}

/// Constant-acceleration transition over `dt` seconds.
pub(crate) fn trans(dt: f64) -> M3 {
    [[1.0, dt, dt * dt / 2.0], [0.0, 1.0, dt], [0.0, 0.0, 1.0]]
}

/// Process noise over `dt` seconds for white jerk of spectral density `q` (m^2/s^5).
pub(crate) fn proc_noise(dt: f64, q: f64) -> M3 {
    let d2 = dt * dt;
    let d3 = d2 * dt;
    let d4 = d3 * dt;
    let d5 = d4 * dt;
    [
        [q * d5 / 20.0, q * d4 / 8.0, q * d3 / 6.0],
        [q * d4 / 8.0, q * d3 / 3.0, q * d2 / 2.0],
        [q * d3 / 6.0, q * d2 / 2.0, q * dt],
    ]
}

/// `F P F' + Q`.
pub(crate) fn predict_cov(f: &M3, p: &M3, q: &M3) -> M3 {
    add(&mul(&mul(f, p), &transpose(f)), q)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    // REQ: OA-TRUTH-058
    #[test]
    fn the_process_noise_is_the_white_jerk_matrix_element_by_element() {
        // q = 0.1, dt = 2: dt^2 = 4, dt^3 = 8, dt^4 = 16, dt^5 = 32.
        let m = proc_noise(2.0, 0.1);
        let want = [
            [0.1 * 32.0 / 20.0, 0.1 * 16.0 / 8.0, 0.1 * 8.0 / 6.0],
            [0.1 * 16.0 / 8.0, 0.1 * 8.0 / 3.0, 0.1 * 4.0 / 2.0],
            [0.1 * 8.0 / 6.0, 0.1 * 4.0 / 2.0, 0.1 * 2.0],
        ];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - want[i][j]).abs() < 1e-15, "({i},{j}): {m:?}");
            }
        }
        // Literal values of the three distinct entries, so the table above is not a copy of the code.
        assert!((m[0][0] - 0.16).abs() < 1e-15);
        assert!((m[1][0] - 0.2).abs() < 1e-15);
        assert!((m[1][1] - 0.8 / 3.0).abs() < 1e-15);
        assert!((m[2][0] - 0.8 / 6.0).abs() < 1e-15);
        assert!((m[2][2] - 0.2).abs() < 1e-15);
    }

    // REQ: OA-TRUTH-058
    #[test]
    fn the_inverse_is_exact_and_a_singular_or_non_finite_matrix_has_none() {
        let m: M3 = [[2.0, 0.0, 0.0], [1.0, 4.0, 0.0], [0.0, 0.0, 5.0]];
        // Lower triangular: inverse [[1/2,0,0],[-1/8,1/4,0],[0,0,1/5]].
        let i = inv(&m).unwrap();
        let want: M3 = [[0.5, 0.0, 0.0], [-0.125, 0.25, 0.0], [0.0, 0.0, 0.2]];
        for r in 0..3 {
            for c in 0..3 {
                assert!((i[r][c] - want[r][c]).abs() < 1e-15, "{i:?}");
            }
        }
        // Rows 1 and 2 are proportional: determinant zero.
        assert!(inv(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [1.0, 1.0, 1.0]]).is_none());
        assert!(inv(&[[0.0; 3]; 3]).is_none());
        assert!(inv(&[[f64::NAN, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
        assert!(inv(&[[f64::INFINITY, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
    }
}
