//! Helpers shared by the fast paths (plain-double evaluation with a proven error bound and a
//! rounding test; see `util::round_test`). The double-double path is the reference they must
//! agree with bit for bit.

use crate::dd::{dd, two_prod, Dd};

/// `(nh + nl) / (dh + dl)` as `(t, tl)` with a relative error below 2^-100, for positive
/// `nh`, `dh` with `nl`, `dl` at most a few ulps of them (operands below 2^996). One division,
/// an exact product and a correction: `t = nh / dh`, the exact remainder
/// `nh + nl - t (dh + dl)` (the subtraction `nh - t dh` is exact: `t dh` is within 2^-52 of
/// `nh`), divided by `dh`. The neglected terms are second order, `rem * dl / dh^2 < 2^-105`.
pub(crate) fn ratio(nh: f64, nl: f64, dh: f64, dl: f64) -> Dd {
    let t = nh / dh;
    let p = two_prod(t, dh);
    let rem = ((nh - p.hi) - p.lo) + (nl - t * dl);
    dd(t, rem / dh)
}
