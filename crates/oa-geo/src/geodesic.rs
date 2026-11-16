//! Geodesics on an ellipsoid of revolution: the direct and inverse problems.
//!
//! The method is Karney, "Algorithms for geodesics", J. Geodesy 87 (2013) 43-55: the geodesic is
//! mapped onto an auxiliary sphere, distance and longitude are expressed as Fourier series in the
//! arc length on that sphere with coefficients that are power series in the parameter `eps`
//! (and the third flattening `n`), here carried to order 6, and the inverse problem is solved by
//! Newton's method on the azimuth with a starting guess from the spherical solution, the
//! short-line approximation or the astroid construction, with bisection as the safeguard.
//! The coefficient tables in `coef.rs` were derived symbolically from the integrals in the paper.

use crate::coef;
use crate::error::GeoError;
use crate::latlon::LatLon;

pub(crate) const TOL0: f64 = f64::EPSILON;
pub(crate) const TOL2: f64 = 1.490_116_119_384_765_6e-8; // sqrt(f64::EPSILON)
pub(crate) const TOLB: f64 = TOL0 * TOL2;
const TINY: f64 = 1.491_668_146_240_041_3e-154; // sqrt(f64::MIN_POSITIVE)
const MAXIT1: u32 = 20;
const MAXIT2: u32 = MAXIT1 + 53 + 10;

/// Result of the inverse problem.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Inverse {
    /// Geodesic distance in metres.
    pub distance_m: f64,
    /// Forward azimuth at the first point, degrees clockwise from north in (-180, 180].
    pub azimuth1_deg: f64,
    /// Forward azimuth at the second point (direction of travel there), same convention.
    pub azimuth2_deg: f64,
}

/// Result of the direct problem.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Direct {
    /// The destination.
    pub position: LatLon,
    /// Forward azimuth at the destination, degrees in (-180, 180].
    pub azimuth2_deg: f64,
}

/// An ellipsoid of revolution plus the precomputed series tables for it. Build once and reuse.
#[derive(Debug, Clone)]
// A7.2: cold struct, size guard waived
pub struct Ellipsoid {
    pub(crate) a: f64,
    pub(crate) f: f64,
    pub(crate) f1: f64,
    pub(crate) e2: f64,
    pub(crate) ep2: f64,
    n: f64,
    pub(crate) b: f64,
    a3x: [f64; 7],
    c3x: [[f64; 7]; 6],
    pub(crate) gl_x: [f64; GL_N],
    pub(crate) gl_w: [f64; GL_N],
}

pub(crate) const GL_N: usize = 8;

/// The WGS-84 ellipsoid (a = 6378137 m, f = 1/298.257223563).
pub fn wgs84() -> Ellipsoid {
    match Ellipsoid::new(6_378_137.0, 1.0 / 298.257_223_563) {
        Ok(e) => e,
        // The constants above are valid; this arm cannot be reached.
        Err(_) => unreachable_wgs84(),
    }
}

#[cold]
fn unreachable_wgs84() -> Ellipsoid {
    // A sphere of the same radius keeps the function total without a panic path.
    let mut e = Ellipsoid::sphere_unchecked(6_378_137.0);
    e.finish_tables();
    e
}

fn polyval(c: &[f64], x: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, &k| acc * x + k)
}

/// Legendre polynomial value and derivative of degree `n` at `x`.
fn legendre(n: usize, x: f64) -> (f64, f64) {
    let mut p0 = 1.0;
    let mut p1 = x;
    for k in 2..=n {
        let kf = k as f64;
        let p2 = ((2.0 * kf - 1.0) * x * p1 - (kf - 1.0) * p0) / kf;
        p0 = p1;
        p1 = p2;
    }
    let nf = n as f64;
    let dp = nf * (x * p1 - p0) / (x * x - 1.0);
    (p1, dp)
}

pub(crate) fn sq(x: f64) -> f64 {
    x * x
}

/// max(0, x) with a positive zero (f64::max leaves the sign of a zero tie unspecified, and the
/// sign selects between pi and -pi in the atan2 calls that follow).
fn max0(x: f64) -> f64 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

fn norm2(x: f64, y: f64) -> (f64, f64) {
    let r = oa_math::hypot(x, y);
    (x / r, y / r)
}

/// sin and cos of an angle in degrees, exact at multiples of 90.
pub(crate) fn sincosd(x: f64) -> (f64, f64) {
    let r = x % 360.0;
    let q = (r / 90.0).round();
    let r = (r - 90.0 * q).to_radians();
    let (s, c) = oa_math::sin_cos(r);
    let (s, c) = match (q as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (s + 0.0, c + 0.0)
}

/// atan2 in degrees with exact results on the axes.
pub(crate) fn atan2d(y: f64, x: f64) -> f64 {
    let (mut y, mut x, mut q) = (y, x, 0);
    if y.abs() > x.abs() {
        std::mem::swap(&mut x, &mut y);
        q = 2;
    }
    if x < 0.0 {
        x = -x;
        q += 1;
    }
    let ang = oa_math::atan2(y, x).to_degrees();
    match q {
        1 => 180.0_f64.copysign(y) - ang,
        2 => 90.0 - ang,
        3 => -90.0 + ang,
        _ => ang,
    }
}

/// Reduces an angle to (-180, 180].
pub(crate) fn ang_normalize(x: f64) -> f64 {
    let mut y = x % 360.0;
    if y <= -180.0 {
        y += 360.0;
    } else if y > 180.0 {
        y -= 360.0;
    }
    y
}

fn remainder360(x: f64) -> f64 {
    let mut r = x % 360.0;
    if r > 180.0 {
        r -= 360.0;
    } else if r < -180.0 {
        r += 360.0;
    }
    r
}

/// Error-free sum: returns (s, t) with s = fl(u + v) and s + t = u + v exactly.
fn two_sum(u: f64, v: f64) -> (f64, f64) {
    let s = u + v;
    let mut up = s - v;
    let mut vpp = s - up;
    up -= u;
    vpp -= v;
    let t = if s != 0.0 { 0.0 - (up + vpp) } else { s };
    (s, t)
}

/// y - x reduced to [-180, 180] with the rounding error returned separately.
fn ang_diff(x: f64, y: f64) -> (f64, f64) {
    let (d, e) = two_sum(remainder360(-x), remainder360(y));
    let (d, e) = two_sum(remainder360(d), e);
    let d = if d == 0.0 || d.abs() == 180.0 {
        d.copysign(if e == 0.0 { y - x } else { -e })
    } else {
        d
    };
    (d, e)
}

/// Rounds values within about 1/16 of the unit roundoff of zero to zero-ish grid values, so that
/// inputs like 1e-20 degrees do not generate spurious tiny angles.
fn ang_round(x: f64) -> f64 {
    let z = 1.0 / 16.0;
    let y = x.abs();
    let w = if y < z { z - (z - y) } else { y };
    w.copysign(x)
}

/// sum_{k=1..6} c[k-1] sin(2 k x) by Clenshaw summation.
fn sin_series(c: &[f64; 6], sinx: f64, cosx: f64) -> f64 {
    let ar = 2.0 * (cosx - sinx) * (cosx + sinx);
    let (mut b1, mut b2) = (0.0, 0.0);
    for &ck in c.iter().rev() {
        let b0 = ck + ar * b1 - b2;
        b2 = b1;
        b1 = b0;
    }
    b1 * 2.0 * sinx * cosx
}

fn eps_from_k2(k2: f64) -> f64 {
    k2 / (2.0 * (1.0 + (1.0 + k2).sqrt()) + k2)
}

fn series6(table: &[&[f64]; 6], eps: f64) -> [f64; 6] {
    let mut out = [0.0; 6];
    for (o, t) in out.iter_mut().zip(table.iter()) {
        *o = polyval(t, eps);
    }
    out
}

fn a1(eps: f64) -> f64 {
    polyval(coef::A1_NUM, eps) / (1.0 - eps)
}

fn a2(eps: f64) -> f64 {
    polyval(coef::A2_NUM, eps) / (1.0 + eps)
}

/// Real root of the astroid quartic used for the near-antipodal starting guess (Karney 2013,
/// section 4): the positive root k of k^4 + 2 k^3 - (x^2 + y^2 - 1) k^2 - 2 y^2 k - y^2 = 0.
fn astroid(x: f64, y: f64) -> f64 {
    let p = sq(x);
    let q = sq(y);
    let r = (p + q - 1.0) / 6.0;
    if q == 0.0 && r <= 0.0 {
        return 0.0;
    }
    let s = p * q / 4.0;
    let r2 = sq(r);
    let r3 = r * r2;
    let disc = s * (s + 2.0 * r3);
    let mut u = r;
    if disc >= 0.0 {
        let mut t3 = s + r3;
        t3 += if t3 < 0.0 { -disc.sqrt() } else { disc.sqrt() };
        let t = oa_math::cbrt(t3);
        u += t + if t != 0.0 { r2 / t } else { 0.0 };
    } else {
        let ang = oa_math::atan2((-disc).sqrt(), -(s + r3));
        u += 2.0 * r * oa_math::cos(ang / 3.0);
    }
    let v = (sq(u) + q).sqrt();
    let uv = if u < 0.0 { q / (v - u) } else { u + v };
    let w = (uv - q) / (2.0 * v);
    uv / ((uv + sq(w)).sqrt() + w)
}

// A7.2: cold struct, size guard waived
struct Lambda12 {
    lam12: f64,
    salp2: f64,
    calp2: f64,
    sig12: f64,
    ssig1: f64,
    csig1: f64,
    ssig2: f64,
    csig2: f64,
    eps: f64,
    dlam12: f64,
}

/// Starting guess for the inverse problem. `sig12 >= 0` means the solution is complete.
// A7.2: cold struct, size guard waived
struct Start {
    sig12: f64,
    salp1: f64,
    calp1: f64,
    salp2: f64,
    calp2: f64,
    dnm: f64,
}

impl Ellipsoid {
    /// Builds an ellipsoid with equatorial radius `a` (metres, positive) and flattening `f` in
    /// [0, 0.01]. Prolate and very oblate figures are outside the range the series support.
    pub fn new(a: f64, f: f64) -> Result<Ellipsoid, GeoError> {
        if !a.is_finite() || a <= 0.0 {
            return Err(GeoError::BadEllipsoid("equatorial radius must be positive"));
        }
        if !f.is_finite() || !(0.0..=0.01).contains(&f) {
            return Err(GeoError::BadEllipsoid("flattening must be in [0, 0.01]"));
        }
        let f1 = 1.0 - f;
        let e2 = f * (2.0 - f);
        let mut e = Ellipsoid {
            a,
            f,
            f1,
            e2,
            ep2: e2 / sq(f1),
            n: f / (2.0 - f),
            b: a * f1,
            a3x: [0.0; 7],
            c3x: [[0.0; 7]; 6],
            gl_x: [0.0; GL_N],
            gl_w: [0.0; GL_N],
        };
        e.finish_tables();
        Ok(e)
    }

    fn sphere_unchecked(a: f64) -> Ellipsoid {
        Ellipsoid {
            a,
            f: 0.0,
            f1: 1.0,
            e2: 0.0,
            ep2: 0.0,
            n: 0.0,
            b: a,
            a3x: [0.0; 7],
            c3x: [[0.0; 7]; 6],
            gl_x: [0.0; GL_N],
            gl_w: [0.0; GL_N],
        }
    }

    // E3 / H2: the loops index fixed-size tables by the loop's own bounds: `j` and `l` walk
    // `coef::A3` and `coef::C3`, which match the 7-element and 6x7 tables, and `i` and `m - 1 - i`
    // lie in 0..GL_N for i < GL_N / 2.
    #[allow(clippy::indexing_slicing)]
    fn finish_tables(&mut self) {
        for (j, row) in coef::A3.iter().enumerate() {
            self.a3x[j] = polyval(row, self.n);
        }
        for (l, rows) in coef::C3.iter().enumerate() {
            for (j, row) in rows.iter().enumerate() {
                self.c3x[l][j] = polyval(row, self.n);
            }
        }
        // Gauss-Legendre nodes and weights by Newton's method on P_n.
        let m = GL_N;
        for i in 0..m / 2 {
            let mut x = oa_math::cos(std::f64::consts::PI * (i as f64 + 0.75) / (m as f64 + 0.5));
            for _ in 0..100 {
                let (p, dp) = legendre(m, x);
                let dx = p / dp;
                x -= dx;
                if dx.abs() < 1e-16 {
                    break;
                }
            }
            let (_, dp) = legendre(m, x);
            let w = 2.0 / ((1.0 - x * x) * dp * dp);
            self.gl_x[i] = -x;
            self.gl_x[m - 1 - i] = x;
            self.gl_w[i] = w;
            self.gl_w[m - 1 - i] = w;
        }
    }

    /// Equatorial radius in metres.
    pub fn equatorial_radius(&self) -> f64 {
        self.a
    }

    /// Flattening.
    pub fn flattening(&self) -> f64 {
        self.f
    }

    fn a3f(&self, eps: f64) -> f64 {
        polyval(&self.a3x, eps)
    }

    fn c3f(&self, eps: f64) -> [f64; 6] {
        let mut out = [0.0; 6];
        for (o, row) in out.iter_mut().zip(self.c3x.iter()) {
            *o = polyval(row, eps);
        }
        out
    }

    /// Reduced length and distance on the auxiliary sphere (both divided by b).
    #[allow(clippy::too_many_arguments)]
    fn lengths(
        eps: f64,
        sig12: f64,
        ssig1: f64,
        csig1: f64,
        dn1: f64,
        ssig2: f64,
        csig2: f64,
        dn2: f64,
    ) -> (f64, f64) {
        let c1 = series6(&coef::C1, eps);
        let c2 = series6(&coef::C2, eps);
        let a1v = a1(eps);
        let a2v = a2(eps);
        let b1 = sin_series(&c1, ssig2, csig2) - sin_series(&c1, ssig1, csig1);
        let b2 = sin_series(&c2, ssig2, csig2) - sin_series(&c2, ssig1, csig1);
        let j12 = (a1v - a2v) * sig12 + (a1v * b1 - a2v * b2);
        let s12b = a1v * (sig12 + b1);
        let m12b = dn2 * (csig1 * ssig2) - dn1 * (ssig1 * csig2) - csig1 * csig2 * j12;
        (s12b, m12b)
    }

    #[allow(clippy::too_many_arguments)]
    fn lambda12(
        &self,
        sbet1: f64,
        cbet1: f64,
        dn1: f64,
        sbet2: f64,
        cbet2: f64,
        dn2: f64,
        salp1: f64,
        calp1: f64,
        slam120: f64,
        clam120: f64,
        diffp: bool,
    ) -> Lambda12 {
        let calp1 = if sbet1 == 0.0 && calp1 == 0.0 {
            -TINY
        } else {
            calp1
        };
        let salp0 = salp1 * cbet1;
        let calp0 = oa_math::hypot(calp1, salp1 * sbet1);
        let mut ssig1 = sbet1;
        let somg1 = salp0 * sbet1;
        let mut csig1 = calp1 * cbet1;
        let comg1 = csig1;
        (ssig1, csig1) = norm2(ssig1, csig1);
        let salp2 = if cbet2 != cbet1 { salp0 / cbet2 } else { salp1 };
        let calp2 = if cbet2 != cbet1 || sbet2.abs() != -sbet1 {
            (sq(calp1 * cbet1)
                + if cbet1 < -sbet1 {
                    (cbet2 - cbet1) * (cbet1 + cbet2)
                } else {
                    (sbet1 - sbet2) * (sbet1 + sbet2)
                })
            .sqrt()
                / cbet2
        } else {
            calp1.abs()
        };
        let mut ssig2 = sbet2;
        let somg2 = salp0 * sbet2;
        let mut csig2 = calp2 * cbet2;
        let comg2 = csig2;
        (ssig2, csig2) = norm2(ssig2, csig2);
        let sig12 = oa_math::atan2(
            max0(csig1 * ssig2 - ssig1 * csig2),
            csig1 * csig2 + ssig1 * ssig2,
        );
        let somg12 = max0(comg1 * somg2 - somg1 * comg2);
        let comg12 = comg1 * comg2 + somg1 * somg2;
        let eta = oa_math::atan2(
            somg12 * clam120 - comg12 * slam120,
            comg12 * clam120 + somg12 * slam120,
        );
        let k2 = sq(calp0) * self.ep2;
        let eps = eps_from_k2(k2);
        let c3 = self.c3f(eps);
        let b312 = sin_series(&c3, ssig2, csig2) - sin_series(&c3, ssig1, csig1);
        let domg12 = -self.f * self.a3f(eps) * salp0 * (sig12 + b312);
        let lam12 = eta + domg12;
        let dlam12 = if !diffp {
            0.0
        } else if calp2 == 0.0 {
            -2.0 * self.f1 * dn1 / sbet1
        } else {
            let (_, m12b) = Self::lengths(eps, sig12, ssig1, csig1, dn1, ssig2, csig2, dn2);
            m12b * self.f1 / (calp2 * cbet2)
        };
        Lambda12 {
            lam12,
            salp2,
            calp2,
            sig12,
            ssig1,
            csig1,
            ssig2,
            csig2,
            eps,
            dlam12,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn inverse_start(
        &self,
        sbet1: f64,
        cbet1: f64,
        sbet2: f64,
        cbet2: f64,
        lam12: f64,
        slam12: f64,
        clam12: f64,
    ) -> Start {
        let mut st = Start {
            sig12: -1.0,
            salp1: 0.0,
            calp1: 0.0,
            salp2: 0.0,
            calp2: 0.0,
            dnm: 0.0,
        };
        let sbet12 = sbet2 * cbet1 - cbet2 * sbet1;
        let cbet12 = cbet2 * cbet1 + sbet2 * sbet1;
        let sbet12a = sbet2 * cbet1 + cbet2 * sbet1;
        let shortline = cbet12 >= 0.0 && sbet12 < 0.5 && cbet2 * lam12 < 0.5;
        let (somg12, comg12);
        if shortline {
            let mut sbetm2 = sq(sbet1 + sbet2);
            sbetm2 /= sbetm2 + sq(cbet1 + cbet2);
            st.dnm = (1.0 + self.ep2 * sbetm2).sqrt();
            let omg12 = lam12 / (self.f1 * st.dnm);
            (somg12, comg12) = oa_math::sin_cos(omg12);
        } else {
            somg12 = slam12;
            comg12 = clam12;
        }
        st.salp1 = cbet2 * somg12;
        st.calp1 = if comg12 >= 0.0 {
            sbet12 + cbet2 * sbet1 * sq(somg12) / (1.0 + comg12)
        } else {
            sbet12a - cbet2 * sbet1 * sq(somg12) / (1.0 - comg12)
        };
        let ssig12 = oa_math::hypot(st.salp1, st.calp1);
        let csig12 = sbet1 * sbet2 + cbet1 * cbet2 * comg12;
        let etol2 =
            0.1 * TOL2 / (self.f.abs().max(0.001) * (1.0_f64).min(1.0 - self.f / 2.0) / 2.0).sqrt();
        if shortline && ssig12 < etol2 {
            st.salp2 = cbet1 * somg12;
            st.calp2 = sbet12
                - cbet1
                    * sbet2
                    * if comg12 >= 0.0 {
                        sq(somg12) / (1.0 + comg12)
                    } else {
                        1.0 - comg12
                    };
            (st.salp2, st.calp2) = norm2(st.salp2, st.calp2);
            st.sig12 = oa_math::atan2(ssig12, csig12);
        } else if self.n.abs() > 0.1
            || csig12 >= 0.0
            || ssig12 >= 6.0 * self.n.abs() * std::f64::consts::PI * sq(cbet1)
        {
            // The zeroth-order spherical estimate is adequate.
        } else {
            let lam12x = oa_math::atan2(-slam12, -clam12);
            let k2 = sq(sbet1) * self.ep2;
            let eps = eps_from_k2(k2);
            let lamscale = self.f * cbet1 * self.a3f(eps) * std::f64::consts::PI;
            let betscale = lamscale * cbet1;
            let x = lam12x / lamscale;
            let y = sbet12a / betscale;
            let k = astroid(x, y);
            let omg12a = lamscale * (-x * k / (1.0 + k));
            let somg12 = oa_math::sin(omg12a);
            let comg12 = -oa_math::cos(omg12a);
            st.salp1 = cbet2 * somg12;
            st.calp1 = sbet12a - cbet2 * sbet1 * sq(somg12) / (1.0 - comg12);
        }
        if st.salp1 > 0.0 || st.salp1.is_nan() {
            (st.salp1, st.calp1) = norm2(st.salp1, st.calp1);
        } else {
            st.salp1 = 1.0;
            st.calp1 = 0.0;
        }
        st
    }

    /// OA-GEO-003/005: solves the inverse problem between two points. Distance in metres and
    /// the forward azimuth at each end in degrees. Nearly antipodal and coincident points are
    /// handled; an error is returned only if the solver fails to converge, which is not
    /// expected for any valid pair.
    pub fn inverse(&self, p1: LatLon, p2: LatLon) -> Result<Inverse, GeoError> {
        let (lat1, lon1, lat2, lon2) = (p1.lat(), p1.lon(), p2.lat(), p2.lon());
        let (lon12, e) = ang_diff(lon1, lon2);
        let lonsign = if lon12.is_sign_negative() { -1.0 } else { 1.0 };
        let lon12 = lon12 * lonsign;
        let e = e * lonsign;
        let lon12s = ang_round((180.0 - lon12) - e);
        let lon12 = ang_round(lon12 + 0.0);
        let lam12 = lon12.to_radians();
        let (slam12, clam12) = if lon12 > 90.0 {
            let (s, c) = sincosd(lon12s);
            (s, -c)
        } else {
            sincosd(lon12)
        };
        let (mut lat1, mut lat2) = (ang_round(lat1), ang_round(lat2));
        let mut lonsign = lonsign;
        let swapp = if lat1.abs() < lat2.abs() {
            std::mem::swap(&mut lat1, &mut lat2);
            lonsign = -lonsign;
            -1.0
        } else {
            1.0
        };
        let latsign = if lat1.is_sign_negative() { 1.0 } else { -1.0 };
        lat1 *= latsign;
        lat2 *= latsign;

        let (mut sbet1, mut cbet1) = sincosd(lat1);
        sbet1 *= self.f1;
        (sbet1, cbet1) = norm2(sbet1, cbet1);
        cbet1 = cbet1.max(TINY);
        let (mut sbet2, mut cbet2) = sincosd(lat2);
        sbet2 *= self.f1;
        (sbet2, cbet2) = norm2(sbet2, cbet2);
        cbet2 = cbet2.max(TINY);
        if cbet1 < -sbet1 {
            if cbet2 == cbet1 {
                sbet2 = sbet1.copysign(sbet2);
            }
        } else if sbet2.abs() == -sbet1 {
            cbet2 = cbet1;
        }
        let dn1 = (1.0 + self.ep2 * sq(sbet1)).sqrt();
        let dn2 = (1.0 + self.ep2 * sq(sbet2)).sqrt();

        let (mut salp1, mut calp1, mut salp2, mut calp2);
        let mut s12x = 0.0;
        let mut meridian = lat1 == -90.0 || slam12 == 0.0;
        salp1 = 0.0;
        calp1 = 0.0;
        salp2 = 0.0;
        calp2 = 0.0;

        if meridian {
            calp1 = clam12;
            salp1 = slam12;
            calp2 = 1.0;
            salp2 = 0.0;
            let ssig1 = sbet1;
            let csig1 = calp1 * cbet1;
            let ssig2 = sbet2;
            let csig2 = calp2 * cbet2;
            let sig12 = oa_math::atan2(
                max0(csig1 * ssig2 - ssig1 * csig2),
                csig1 * csig2 + ssig1 * ssig2,
            );
            // On a meridian eps equals the third flattening n exactly.
            let (mut s12b, m12b) =
                Self::lengths(self.n, sig12, ssig1, csig1, dn1, ssig2, csig2, dn2);
            if sig12 < 1.0 || m12b >= 0.0 {
                if sig12 < 3.0 * TINY || (sig12 < TOL0 && (s12b < 0.0 || m12b < 0.0)) {
                    s12b = 0.0;
                }
                s12x = s12b * self.b;
            } else {
                meridian = false;
            }
        }

        if !meridian && sbet1 == 0.0 && (self.f <= 0.0 || lon12s >= self.f * 180.0) {
            calp1 = 0.0;
            calp2 = 0.0;
            salp1 = 1.0;
            salp2 = 1.0;
            s12x = self.a * lam12;
        } else if !meridian {
            let st = self.inverse_start(sbet1, cbet1, sbet2, cbet2, lam12, slam12, clam12);
            if st.sig12 >= 0.0 {
                s12x = st.sig12 * self.b * st.dnm;
                salp1 = st.salp1;
                calp1 = st.calp1;
                salp2 = st.salp2;
                calp2 = st.calp2;
            } else {
                salp1 = st.salp1;
                calp1 = st.calp1;
                let mut tripn = false;
                let mut tripb = false;
                let (mut salp1a, mut calp1a, mut salp1b, mut calp1b) = (TINY, 1.0, TINY, -1.0);
                let mut last: Option<Lambda12> = None;
                for numit in 0..MAXIT2 {
                    let cur = self.lambda12(
                        sbet1,
                        cbet1,
                        dn1,
                        sbet2,
                        cbet2,
                        dn2,
                        salp1,
                        calp1,
                        slam12,
                        clam12,
                        numit < MAXIT1,
                    );
                    let v = cur.lam12;
                    let dv = cur.dlam12;
                    last = Some(cur);
                    // Written so that NaN exits the loop.
                    if tripb || v.is_nan() || v.abs() < (if tripn { 8.0 } else { 1.0 }) * TOL0 {
                        break;
                    }
                    if v > 0.0 && (numit > MAXIT1 || calp1 / salp1 > calp1b / salp1b) {
                        salp1b = salp1;
                        calp1b = calp1;
                    } else if v < 0.0 && (numit > MAXIT1 || calp1 / salp1 < calp1a / salp1a) {
                        salp1a = salp1;
                        calp1a = calp1;
                    }
                    if numit < MAXIT1 && dv > 0.0 {
                        let dalp1 = -v / dv;
                        if dalp1.abs() < std::f64::consts::PI {
                            let (sdalp1, cdalp1) = oa_math::sin_cos(dalp1);
                            let nsalp1 = salp1 * cdalp1 + calp1 * sdalp1;
                            if nsalp1 > 0.0 {
                                calp1 = calp1 * cdalp1 - salp1 * sdalp1;
                                salp1 = nsalp1;
                                (salp1, calp1) = norm2(salp1, calp1);
                                tripn = v.abs() <= 16.0 * TOL0;
                                continue;
                            }
                        }
                    }
                    salp1 = (salp1a + salp1b) / 2.0;
                    calp1 = (calp1a + calp1b) / 2.0;
                    (salp1, calp1) = norm2(salp1, calp1);
                    tripn = false;
                    tripb = (salp1a - salp1).abs() + (calp1a - calp1) < TOLB
                        || (salp1 - salp1b).abs() + (calp1 - calp1b) < TOLB;
                }
                let last = match last {
                    Some(l) if l.lam12.is_finite() => l,
                    _ => return Err(GeoError::NoConvergence("inverse problem")),
                };
                let (s12b, _) = Self::lengths(
                    last.eps, last.sig12, last.ssig1, last.csig1, dn1, last.ssig2, last.csig2, dn2,
                );
                s12x = s12b * self.b;
                salp2 = last.salp2;
                calp2 = last.calp2;
            }
        }

        if swapp < 0.0 {
            std::mem::swap(&mut salp1, &mut salp2);
            std::mem::swap(&mut calp1, &mut calp2);
        }
        salp1 *= swapp * lonsign;
        calp1 *= swapp * latsign;
        salp2 *= swapp * lonsign;
        calp2 *= swapp * latsign;
        let out = Inverse {
            distance_m: s12x + 0.0,
            azimuth1_deg: atan2d(salp1, calp1),
            azimuth2_deg: atan2d(salp2, calp2),
        };
        if out.distance_m.is_finite()
            && out.azimuth1_deg.is_finite()
            && out.azimuth2_deg.is_finite()
        {
            Ok(out)
        } else {
            Err(GeoError::NoConvergence("inverse problem"))
        }
    }

    /// OA-GEO-004: solves the direct problem. From `p1`, travel `distance_m` metres along the
    /// geodesic with initial azimuth `azimuth_deg`. Any finite azimuth is accepted (it is
    /// reduced modulo 360) and any finite distance, including negative (travel backwards) and
    /// more than one circuit.
    pub fn direct(
        &self,
        p1: LatLon,
        azimuth_deg: f64,
        distance_m: f64,
    ) -> Result<Direct, GeoError> {
        if !azimuth_deg.is_finite() {
            return Err(GeoError::NonFinite("azimuth"));
        }
        if !distance_m.is_finite() {
            return Err(GeoError::NonFinite("distance"));
        }
        let (lat2, lon12, azi2) = self.direct_unrolled(p1.lat(), azimuth_deg, distance_m);
        if !(lat2.is_finite() && lon12.is_finite() && azi2.is_finite()) {
            return Err(GeoError::NoConvergence("direct problem"));
        }
        let lat2 = lat2.clamp(-90.0, 90.0);
        let lon2 = ang_normalize(p1.lon() + ang_normalize(lon12));
        let position = LatLon::new(lat2, lon2)?;
        Ok(Direct {
            position,
            azimuth2_deg: azi2,
        })
    }

    /// Direct problem returning latitude, the signed longitude change in degrees *without*
    /// reduction to (-180, 180] (so it grows along an east- or west-going geodesic), and the
    /// forward azimuth. Does not validate its inputs.
    pub(crate) fn direct_unrolled(&self, lat1: f64, azi1: f64, s12: f64) -> (f64, f64, f64) {
        let azi1 = ang_round(ang_normalize(azi1));
        let (salp1, calp1) = sincosd(azi1);
        let (mut sbet1, mut cbet1) = sincosd(ang_round(lat1));
        sbet1 *= self.f1;
        (sbet1, cbet1) = norm2(sbet1, cbet1);
        cbet1 = cbet1.max(TINY);
        let salp0 = salp1 * cbet1;
        let calp0 = oa_math::hypot(calp1, salp1 * sbet1);
        let mut ssig1 = sbet1;
        let somg1 = salp0 * sbet1;
        let mut csig1 = if sbet1 != 0.0 || calp1 != 0.0 {
            cbet1 * calp1
        } else {
            1.0
        };
        let comg1 = csig1;
        (ssig1, csig1) = norm2(ssig1, csig1);
        let k2 = sq(calp0) * self.ep2;
        let eps = eps_from_k2(k2);
        let a1m = a1(eps);
        let c1 = series6(&coef::C1, eps);
        let b11 = sin_series(&c1, ssig1, csig1);
        let (s, c) = oa_math::sin_cos(b11);
        let stau1 = ssig1 * c + csig1 * s;
        let ctau1 = csig1 * c - ssig1 * s;
        let c1p = series6(&coef::C1P, eps);
        let c3 = self.c3f(eps);
        let a3c = -self.f * salp0 * self.a3f(eps);
        let b31 = sin_series(&c3, ssig1, csig1);

        let tau12 = s12 / (self.b * a1m);
        let (s, c) = oa_math::sin_cos(tau12);
        let b12 = -sin_series(&c1p, stau1 * c + ctau1 * s, ctau1 * c - stau1 * s);
        let sig12 = tau12 - (b12 - b11);
        let (ssig12, csig12) = oa_math::sin_cos(sig12);
        let ssig2 = ssig1 * csig12 + csig1 * ssig12;
        let csig2 = csig1 * csig12 - ssig1 * ssig12;
        let sbet2 = calp0 * ssig2;
        let mut cbet2 = oa_math::hypot(salp0, calp0 * csig2);
        let mut csig2c = csig2;
        if cbet2 == 0.0 {
            cbet2 = TINY;
            csig2c = TINY;
        }
        let salp2 = salp0;
        let calp2 = calp0 * csig2c;
        let somg2 = salp0 * ssig2;
        let comg2 = csig2c;
        let e = 1.0_f64.copysign(salp0);
        let omg12 = e
            * (sig12 - (oa_math::atan2(ssig2, csig2c) - oa_math::atan2(ssig1, csig1))
                + (oa_math::atan2(e * somg2, comg2) - oa_math::atan2(e * somg1, comg1)));
        let lam12 = omg12 + a3c * (sig12 + (sin_series(&c3, ssig2, csig2c) - b31));
        let lat2 = atan2d(sbet2, self.f1 * cbet2);
        let azi2 = atan2d(salp2, calp2);
        (lat2, lam12.to_degrees(), azi2)
    }
}

#[cfg(test)]
mod tests {
    use super::{ang_diff, ang_normalize, ang_round, atan2d, remainder360, sincosd, two_sum};

    // REQ: OA-GEO-020
    #[test]
    fn degree_trig_is_exact_at_quadrant_angles() {
        assert_eq!(sincosd(0.0), (0.0, 1.0));
        assert_eq!(sincosd(90.0), (1.0, 0.0));
        assert_eq!(sincosd(180.0), (0.0, -1.0));
        assert_eq!(sincosd(-90.0), (-1.0, 0.0));
        assert_eq!(sincosd(360.0), (0.0, 1.0));
        assert_eq!(sincosd(180.0).0.to_bits(), 0.0f64.to_bits());
        assert_eq!(sincosd(270.0).1.to_bits(), 0.0f64.to_bits());
        assert_eq!(sincosd(90.0).1.to_bits(), 0.0f64.to_bits());
    }

    // REQ: OA-GEO-021
    #[test]
    fn atan2_degrees_has_exact_axis_and_diagonal_values() {
        assert_eq!(atan2d(0.0, 1.0), 0.0);
        assert_eq!(atan2d(1.0, 0.0), 90.0);
        assert_eq!(atan2d(0.0, -1.0), 180.0);
        assert_eq!(atan2d(-1.0, 0.0), -90.0);
        assert_eq!(atan2d(1.0, 1.0), 45.0);
        assert_eq!(atan2d(1.0, -1.0), 135.0);
        assert_eq!(atan2d(-1.0, -1.0), -135.0);
        assert_eq!(atan2d(-0.0, 1.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(atan2d(-0.0, -1.0), -180.0);
    }

    // REQ: OA-GEO-022
    #[test]
    fn angle_reductions_have_explicit_half_turn_endpoints() {
        assert_eq!(ang_normalize(-180.0), 180.0);
        assert_eq!(ang_normalize(180.0), 180.0);
        assert_eq!(ang_normalize(540.0), 180.0);
        assert_eq!(remainder360(-180.0), -180.0);
        assert_eq!(remainder360(180.0), 180.0);
        assert_eq!(remainder360(540.0), 180.0);
    }

    // REQ: OA-GEO-023
    #[test]
    fn angle_difference_preserves_direction_at_half_turns() {
        assert_eq!(ang_diff(0.0, 180.0), (180.0, 0.0));
        assert_eq!(ang_diff(0.0, -180.0), (-180.0, 0.0));
        assert_eq!(ang_diff(180.0, 0.0), (-180.0, 0.0));
    }

    // REQ: OA-GEO-024
    #[test]
    fn compensated_sum_returns_the_lost_unit() {
        assert_eq!(two_sum(1.0e16, 1.0), (1.0e16, 1.0));
        assert_eq!(two_sum(1.0e16, -1.0e16), (0.0, 0.0));
    }

    // REQ: OA-GEO-025
    #[test]
    fn angle_rounding_cleans_sub_grid_noise_and_preserves_its_sign() {
        assert_eq!(ang_round(1.0e-20), 0.0);
        assert_eq!(ang_round(-1.0e-20f64).to_bits(), (-0.0f64).to_bits());
        assert_eq!(ang_round(1.0e-17), 2.0f64.powi(-57));
    }
}
