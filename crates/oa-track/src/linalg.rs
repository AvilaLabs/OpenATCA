//! 5 x 5 linear algebra for the horizontal filter. Core (pure). Fixed-size arrays and loops
//! whose bounds are the constant [`N`], written in a fixed order (part of the determinism
//! guarantee). Indexing here is bounded by the array type itself (the same exception oa-sha256
//! and oa-deflate record for fixed-size tables), so the module opts out of the indexing lint.
#![allow(clippy::indexing_slicing, clippy::needless_range_loop)]

/// State dimension of the horizontal filter: east, north, east velocity, north velocity, turn
/// rate.
pub(crate) const N: usize = 5;
/// A state vector.
pub(crate) type V5 = [f64; N];
/// A matrix, row major.
pub(crate) type M5 = [[f64; N]; N];

pub(crate) const ZERO: M5 = [[0.0; N]; N];

pub(crate) fn ident() -> M5 {
    let mut m = ZERO;
    for i in 0..N {
        m[i][i] = 1.0;
    }
    m
}

pub(crate) fn mul(a: &M5, b: &M5) -> M5 {
    let mut o = ZERO;
    for i in 0..N {
        for k in 0..N {
            let aik = a[i][k];
            for j in 0..N {
                o[i][j] += aik * b[k][j];
            }
        }
    }
    o
}

pub(crate) fn transpose(a: &M5) -> M5 {
    let mut o = ZERO;
    for i in 0..N {
        for j in 0..N {
            o[j][i] = a[i][j];
        }
    }
    o
}

pub(crate) fn add(a: &M5, b: &M5) -> M5 {
    let mut o = ZERO;
    for i in 0..N {
        for j in 0..N {
            o[i][j] = a[i][j] + b[i][j];
        }
    }
    o
}

pub(crate) fn scale(a: &M5, s: f64) -> M5 {
    let mut o = *a;
    for r in o.iter_mut() {
        for v in r.iter_mut() {
            *v *= s;
        }
    }
    o
}

pub(crate) fn matvec(a: &M5, v: &V5) -> V5 {
    let mut o = [0.0; N];
    for i in 0..N {
        let mut s = 0.0;
        for j in 0..N {
            s += a[i][j] * v[j];
        }
        o[i] = s;
    }
    o
}

/// `a` with the off-diagonal pairs averaged, so rounding cannot make a covariance asymmetric.
pub(crate) fn symmetrise(a: &M5) -> M5 {
    let mut o = *a;
    for i in 0..N {
        for j in (i + 1)..N {
            let m = (a[i][j] + a[j][i]) / 2.0;
            o[i][j] = m;
            o[j][i] = m;
        }
    }
    o
}

/// `v v'`.
pub(crate) fn outer(v: &V5) -> M5 {
    let mut o = ZERO;
    for i in 0..N {
        for j in 0..N {
            o[i][j] = v[i] * v[j];
        }
    }
    o
}

/// `F P F'`.
pub(crate) fn sandwich(f: &M5, p: &M5) -> M5 {
    mul(&mul(f, p), &transpose(f))
}
