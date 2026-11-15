//! Double-double arithmetic built from `+ - * /` only (no fused multiply-add, no libm).
//!
//! A [`Dd`] is an unevaluated sum `hi + lo` with `|lo| <= ulp(hi) / 2`, about 106 bits. The
//! error-free transformations are the classical ones of Knuth, Dekker and Veltkamp. Products
//! split their operands with the Veltkamp constant, so an operand must stay below 2^996 in
//! magnitude; every caller in this crate scales its operands into range first.

/// An unevaluated sum of two doubles (plain data; the arithmetic is in free functions).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Dd {
    pub hi: f64,
    pub lo: f64,
}

pub(crate) const fn dd(hi: f64, lo: f64) -> Dd {
    Dd { hi, lo }
}

/// Veltkamp splitter, 2^27 + 1.
const SPLITTER: f64 = 134_217_729.0;

/// Exact sum: `hi + lo == a + b`, `hi = fl(a + b)`.
pub(crate) fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    let err = (a - (s - bb)) + (b - bb);
    dd(s, err)
}

/// Exact sum when `|a| >= |b|` (or `a == 0`).
pub(crate) fn fast_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let err = b - (s - a);
    dd(s, err)
}

/// Exact product: `hi + lo == a * b`, `hi = fl(a * b)` (no overflow or underflow in the parts).
pub(crate) fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    let ta = SPLITTER * a;
    let ah = ta - (ta - a);
    let al = a - ah;
    let tb = SPLITTER * b;
    let bh = tb - (tb - b);
    let bl = b - bh;
    let err = ((ah * bh - p) + ah * bl + al * bh) + al * bl;
    dd(p, err)
}

pub(crate) fn neg(a: Dd) -> Dd {
    dd(-a.hi, -a.lo)
}

/// Sum of two double-doubles (relative error about 2^-104).
pub(crate) fn add(a: Dd, b: Dd) -> Dd {
    let s = two_sum(a.hi, b.hi);
    let t = two_sum(a.lo, b.lo);
    let e = s.lo + t.hi;
    let u = fast_two_sum(s.hi, e);
    let e2 = u.lo + t.lo;
    fast_two_sum(u.hi, e2)
}

pub(crate) fn sub(a: Dd, b: Dd) -> Dd {
    add(a, neg(b))
}

pub(crate) fn add_d(a: Dd, b: f64) -> Dd {
    let s = two_sum(a.hi, b);
    let e = s.lo + a.lo;
    fast_two_sum(s.hi, e)
}

pub(crate) fn mul(a: Dd, b: Dd) -> Dd {
    let p = two_prod(a.hi, b.hi);
    let e = p.lo + (a.hi * b.lo + a.lo * b.hi);
    fast_two_sum(p.hi, e)
}

pub(crate) fn mul_d(a: Dd, b: f64) -> Dd {
    let p = two_prod(a.hi, b);
    let e = p.lo + a.lo * b;
    fast_two_sum(p.hi, e)
}

/// Quotient `a / b` by three rounds of long division (relative error about 2^-104).
pub(crate) fn div(a: Dd, b: Dd) -> Dd {
    let q1 = a.hi / b.hi;
    let r = sub(a, mul_d(b, q1));
    let q2 = r.hi / b.hi;
    let r2 = sub(r, mul_d(b, q2));
    let q3 = r2.hi / b.hi;
    let s = fast_two_sum(q1, q2);
    add_d(s, q3)
}

/// Square root of a positive double-double (relative error about 2^-104); zero gives zero.
pub(crate) fn sqrt(a: Dd) -> Dd {
    if a.hi <= 0.0 {
        return dd(0.0, 0.0);
    }
    let s = a.hi.sqrt();
    let rem = sub(a, two_prod(s, s));
    let corr = rem.hi / (2.0 * s);
    fast_two_sum(s, corr)
}
