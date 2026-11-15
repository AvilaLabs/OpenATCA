#!/usr/bin/env python3
"""Generates the reference data for crates/oa-math/tests/reference.rs.

For each function it writes crates/oa-math/tests/data/<name>.bin: a sequence of records of
little-endian u64 words, [x_bits, (y_bits,) hi_bits, lo_bits], where hi is the correctly
rounded double of the true value (mpmath, 80 digits, rechecked at 140 digits) and lo is the
double nearest to true - hi, so the Rust test can measure the true error of a result.
The inputs are random in log scale over each domain plus edge cases (near multiples of pi/2
including the hardest cases for argument reduction, near 1 for ln/asin/acos, near overflow and
underflow for exp/pow, subnormals, zeros, huge arguments). Seeded, so the output is repeatable.

Dev tool only (mpmath 1.3.0); nothing here is a build dependency.
Usage: python3 tools/oa-math-ref/gen_ref.py crates/oa-math/tests/data
For a larger one-off run set OA_MATH_REF_N and OA_MATH_REF_SEED and point the test at the output
directory with OA_MATH_REF_DIR.
"""
import math
import os
import random
import struct
import sys
from fractions import Fraction

from mpmath import mp, mpf, sin, cos, tan, atan, atan2, asin, acos, exp, log, cbrt, sqrt, power, pi, floor

N = int(os.environ.get("OA_MATH_REF_N", 3200))
rng = random.Random(int(os.environ.get("OA_MATH_REF_SEED", 0x0A7CA)))
mp.dps = 80

DBL_MAX = float.fromhex("0x1.fffffffffffffp1023")
OVERFLOW_BOUND = mpf(2) ** 1024 - mpf(2) ** 970  # rounds to infinity from here


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def rnd(v):
    """Correctly rounded double of the mpf v, and the double nearest the remainder."""
    if v == 0:
        return 0.0, 0.0
    a = abs(v)
    if a >= OVERFLOW_BOUND:
        return math.copysign(math.inf, float(v)), 0.0
    if a < mpf(2) ** -1022:
        n = int(mp.nint(a * mpf(2) ** 1074))
        hi = math.copysign(math.ldexp(float(n), -1074), float(v))
        return hi, float(v - mpf(hi))
    hi = float(v)
    return hi, float(v - mpf(hi))


def mag(emin, emax):
    return math.ldexp(1 + rng.random(), rng.randint(emin, emax))


def sgn(x):
    return -x if rng.random() < 0.5 else x


def ulps_away(x, k):
    for _ in range(abs(k)):
        x = math.nextafter(x, math.inf if k > 0 else -math.inf)
    return x


def hard_reduction_args():
    """Per exponent, the 53-bit mantissa M whose x = M 2^e is closest to a multiple of pi/2,
    from the continued fraction of 2^e * 2/pi (its convergent denominators)."""
    mp.prec = 3600
    inv = Fraction(int(mp.floor(2 / pi * mpf(2) ** 3400)), 2 ** 3400)
    mp.dps = 80
    out = []
    for e in range(-52, 972, 2):
        alpha = inv * Fraction(2) ** e
        p, q = alpha.numerator, alpha.denominator
        # continued fraction expansion; track convergent denominators
        h0, h1 = 1, int(p // q)
        k0, k1 = 0, 1
        a, b = q, p - (p // q) * q
        best = 1
        while b != 0:
            t = a // b
            a, b = b, a - t * b
            k0, k1 = k1, t * k1 + k0
            if k1 >= 2 ** 53:
                break
            best = k1
        if best < 2 ** 52:
            t = -(-(2 ** 52) // best)
            m = best * t
        else:
            m = best
        if 2 ** 52 <= m < 2 ** 53:
            out.append(math.ldexp(float(m), e))
    out.append(math.ldexp(6381956970095103.0, 797))  # classic worst case
    return out


HARD = hard_reduction_args()


def trig_inputs():
    xs = [0.0, -0.0, 5e-324, -5e-324, 2.2250738585072014e-308, 1.0, -1.0, math.pi, math.pi / 2,
          math.pi / 4, 2 ** 19, 2 ** 19 + 1.0, DBL_MAX, -DBL_MAX, 2.0 ** 1023, 2.0 ** 52, 1e22, 1e300]
    for e in range(-1074, 1024, 13):
        xs.append(math.ldexp(1.0, e))
    xs += [sgn(h) for h in HARD]
    xs += [ulps_away(h, rng.choice((-2, -1, 1, 2))) for h in HARD[:200]]
    while len(xs) < N:
        r = rng.random()
        if r < 0.30:
            xs.append(sgn(mag(-30, 6)))
        elif r < 0.45:
            xs.append(sgn(mag(-1074, 1023)))
        elif r < 0.60:
            xs.append(rng.uniform(-100, 100))
        elif r < 0.80:
            k = math.ldexp(1 + rng.random(), rng.randint(0, 52))
            x = float(mpf(math.floor(k)) * pi / 2)
            xs.append(sgn(ulps_away(x, rng.randint(-3, 3))))
        else:
            xs.append(sgn(mag(-8, 22)))
    return xs[:N] if len(xs) > N else xs


def atan_inputs():
    xs = [0.0, -0.0, 5e-324, 1.0, -1.0, 1e300, -1e300, 2.0 ** 60, 2.0 ** -27, 0.5, 2.0 ** 61]
    while len(xs) < N:
        r = rng.random()
        if r < 0.35:
            xs.append(sgn(mag(-60, 60)))
        elif r < 0.55:
            xs.append(sgn(mag(-1074, 1023)))
        elif r < 0.85:
            xs.append(rng.uniform(-3, 3))
        else:
            xs.append(ulps_away(sgn(1.0), rng.randint(-5, 5)))
    return xs


def asin_inputs():
    xs = [0.0, -0.0, 1.0, -1.0, 5e-324, 0.5, -0.5, math.sqrt(0.5), 2.0 ** -27, 1 - 2.0 ** -53]
    while len(xs) < N:
        r = rng.random()
        if r < 0.40:
            xs.append(rng.uniform(-1, 1))
        elif r < 0.75:
            k = rng.randint(1, 53)
            xs.append(sgn(1.0 - math.ldexp(rng.random(), -k)))
        elif r < 0.95:
            xs.append(sgn(mag(-70, -1)))
        else:
            xs.append(sgn(ulps_away(rng.choice((0.5, math.sqrt(0.5), 0.25)), rng.randint(-3, 3))))
    return [x for x in xs if abs(x) <= 1.0]


def exp_inputs():
    xs = [0.0, -0.0, 1.0, -1.0, 709.782712893384, 709.7827128933839, -745.1332191019411,
          -745.1332191019412, -708.3964185322641, -745.0, -744.0, 5e-324, 1e-300, 0.5, math.log(2)]
    while len(xs) < N:
        r = rng.random()
        if r < 0.35:
            xs.append(rng.uniform(-745.2, 709.79))
        elif r < 0.55:
            xs.append(rng.uniform(-50, 50))
        elif r < 0.75:
            xs.append(sgn(mag(-70, -1)))
        elif r < 0.85:
            xs.append(rng.uniform(-745.2, -708))
        elif r < 0.93:
            xs.append(ulps_away(rng.choice((709.782712893384, -745.1332191019411, -708.3964185322641)),
                                rng.randint(-20, 20)))
        else:
            k = rng.randint(-1075, 1024)
            xs.append(ulps_away(float(mpf(k) * log(2)), rng.randint(-3, 3)))
    return xs


def ln_inputs():
    xs = [1.0, 5e-324, 2.2250738585072014e-308, DBL_MAX, 2.0, 0.5, 10.0, math.e, math.sqrt(2)]
    while len(xs) < N:
        r = rng.random()
        if r < 0.25:
            xs.append(mag(-1074, 1023))
        elif r < 0.55:
            k = rng.randint(1, 53)
            x = 1.0 + (-1) ** rng.randint(0, 1) * math.ldexp(rng.random(), -k)
            xs.append(x)
        elif r < 0.75:
            xs.append(rng.uniform(0.01, 100))
        elif r < 0.9:
            xs.append(mag(-10, 10))
        else:
            c = rng.choice((math.sqrt(2), 1.5, 0.75, 1 + rng.randint(0, 64) / 64, 2.0 ** rng.randint(-5, 5)))
            xs.append(ulps_away(c, rng.randint(-4, 4)))
    return xs


def cbrt_inputs():
    xs = [0.0, -0.0, 1.0, -1.0, 8.0, 27.0, -27.0, 5e-324, -5e-324, DBL_MAX, 2.0 ** -1022]
    while len(xs) < N:
        r = rng.random()
        if r < 0.5:
            xs.append(sgn(mag(-1074, 1023)))
        elif r < 0.7:
            n = rng.randint(1, 2 ** rng.randint(1, 17))
            xs.append(sgn(ulps_away(float(n) ** 3, rng.randint(-1, 1))))
        else:
            xs.append(rng.uniform(-100, 100))
    return xs


def atan2_inputs():
    ps = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (5e-324, 5e-324), (5e-324, -DBL_MAX),
          (DBL_MAX, 5e-324), (DBL_MAX, DBL_MAX), (1.0, 2.0 ** 60), (2.0 ** 60, 1.0), (1.0, 2.0 ** -60)]
    while len(ps) < N:
        r = rng.random()
        if r < 0.25:
            ps.append((sgn(mag(-1074, 1023)), sgn(mag(-1074, 1023))))
        elif r < 0.65:
            ps.append((sgn(mag(-20, 20)), sgn(mag(-20, 20))))
        elif r < 0.80:
            e = rng.randint(-960, 960)
            ps.append((sgn(math.ldexp(1 + rng.random(), e)), sgn(math.ldexp(1 + rng.random(), e + rng.randint(-62, 62)))))
        else:
            e = rng.randint(-960, 960)
            a = math.ldexp(1 + rng.random(), e)
            ps.append((sgn(ulps_away(a, rng.randint(-2, 2))), sgn(a)))
    return ps


def hypot_inputs():
    ps = [(3.0, 4.0), (5e-324, 5e-324), (DBL_MAX, DBL_MAX), (DBL_MAX, 1.0), (1.0, 2.0 ** -60), (0.0, 5.0),
          (-5.0, 0.0), (2.0 ** 1023, 2.0 ** 1023), (2.0 ** -1022, 2.0 ** -1022), (1.0, 1.0)]
    while len(ps) < N:
        r = rng.random()
        if r < 0.25:
            ps.append((sgn(mag(-1074, 1023)), sgn(mag(-1074, 1023))))
        elif r < 0.55:
            ps.append((sgn(mag(-30, 30)), sgn(mag(-30, 30))))
        elif r < 0.75:
            e = rng.randint(-960, 960)
            ps.append((sgn(math.ldexp(1 + rng.random(), e)), sgn(math.ldexp(1 + rng.random(), e + rng.randint(-60, 0)))))
        elif r < 0.88:
            ps.append((sgn(mag(1020, 1023)), sgn(mag(1020, 1023))))
        else:
            ps.append((sgn(mag(-1074, -1020)), sgn(mag(-1074, -1020))))
    return ps


def pow_inputs():
    ps = [(2.0, 10.0), (2.0, -1074.0), (2.0, 1023.0), (2.0, 1024.0), (10.0, -323.0), (10.0, 308.0),
          (0.5, 1074.0), (-2.0, 3.0), (-2.0, -3.0), (-8.0, 1.0 / 3.0 * 0 + 3.0), (1.0000000000000002, 2.0 ** 52),
          (3.0, 2.0), (5e-324, 0.5), (DBL_MAX, 1.0), (DBL_MAX, 0.5), (4.0, 0.5), (9.0, 0.5), (2.0, 0.5)]
    while len(ps) < N:
        r = rng.random()
        if r < 0.30:
            x = mag(-1074, 1023)
            t = rng.uniform(-745.5, 710.5)
            y = t / math.log(x) if x != 1.0 else 3.0
            ps.append((x, y))
        elif r < 0.50:
            ps.append((mag(-5, 5), rng.uniform(-50, 50)))
        elif r < 0.60:
            ps.append((-mag(-5, 5), float(rng.randint(-60, 60))))
        elif r < 0.75:
            k = rng.randint(1, 53)
            x = 1.0 + (-1) ** rng.randint(0, 1) * math.ldexp(rng.random(), -k)
            if x == 1.0:
                continue
            t = rng.uniform(-745, 709)
            y = t / math.log(x)
            ps.append((x, y))
        elif r < 0.82:
            ps.append((rng.choice((2.0, 10.0, math.e, 0.5, 3.0, 7.0)), rng.choice((0.5, 1 / 3, rng.uniform(-300, 300), float(rng.randint(-1000, 1000))))))
        elif r < 0.90:
            x = mag(-30, 30)
            ps.append((x, rng.uniform(-745.2, -708) / math.log(x) if abs(math.log(x)) > 1e-3 else 2.5))
        else:
            ps.append((float(rng.randint(2, 30)), float(rng.randint(-60, 60))))
    out = []
    for x, y in ps:
        if x == 0 or math.isnan(y) or math.isinf(y) or (x < 0 and y != math.floor(y)):
            continue
        out.append((x, y))
    return out


def ref(fn, args):
    mp.dps = 80
    v1 = fn(*[mpf(a) for a in args])
    mp.dps = 140
    v2 = fn(*[mpf(a) for a in args])
    mp.dps = 80
    h1, l1 = rnd(v1)
    h2, _ = rnd(v2)
    assert h1 == h2 or (h1 != h1 and h2 != h2), (args, h1, h2)
    return h1, l1


FUNCS = {
    "sin": (sin, trig_inputs, 1),
    "cos": (cos, trig_inputs, 1),
    "tan": (tan, trig_inputs, 1),
    "atan": (atan, atan_inputs, 1),
    "asin": (asin, asin_inputs, 1),
    "acos": (acos, asin_inputs, 1),
    "exp": (exp, exp_inputs, 1),
    "ln": (log, ln_inputs, 1),
    "cbrt": (lambda a: -cbrt(-a) if a < 0 else cbrt(a), cbrt_inputs, 1),
    "atan2": (atan2, atan2_inputs, 2),
    "hypot": (lambda a, b: sqrt(a * a + b * b), hypot_inputs, 2),
    "pow": (power, pow_inputs, 2),
}


def main():
    outdir = sys.argv[1]
    for name, (fn, gen, arity) in FUNCS.items():
        recs = bytearray()
        count = 0
        for inp in gen():
            args = inp if arity == 2 else (inp,)
            if any(a == 0 and math.copysign(1.0, a) < 0 for a in args):
                continue  # signed zeros are covered by the special-value tests
            hi, lo = ref(fn, args)
            if hi != hi:
                continue
            words = [bits(a) for a in args] + [bits(hi), bits(lo)]
            recs += struct.pack("<%dQ" % len(words), *words)
            count += 1
        with open("%s/%s.bin" % (outdir, name), "wb") as f:
            f.write(recs)
        print(name, count, len(recs))


main()
