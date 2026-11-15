#!/usr/bin/env python3
"""Generates crates/oa-math/tests/data/cw_hard.bin: the hardest arguments below 2^19 for the
Cody-Waite reduction of sin, cos and tan (the arguments closest to a multiple of pi/2).

For every binade 2^k <= x < 2^19 it takes the multiples of the convergent denominators of the
continued fraction of 2^e * 2/pi (the mantissas M whose x = M 2^e lies closest to a multiple of
pi/2) plus the doubles around the multiples k pi/2 for k up to 3000, 1e5 and 1e6, keeps the 600
with the smallest min(|sin x|, |cos x|) (the closest is about 2^-60.5), and writes records of
little-endian u64 words [x, sin_hi, cos_hi, tan_hi], each the correctly rounded double
(mpmath, 400 bits).

Dev tool only (mpmath 1.3.0). Usage: python3 tools/oa-math-ref/gen_hard_cw.py crates/oa-math/tests/data
"""
import math
import struct
import sys
from fractions import Fraction

from mpmath import mp, mpf, pi, sin, cos, tan

KEEP = 600

mp.prec = 3600
inv = Fraction(int(mp.floor(2 / pi * mpf(2) ** 3400)), 2 ** 3400)
xs = set()
for e in range(-52, -33):  # x = M 2^e in [1, 2^19]
    alpha = inv * Fraction(2) ** e
    p, q = alpha.numerator, alpha.denominator
    a, b = q, p - (p // q) * q
    k0, k1 = 0, 1
    dens = []
    while b != 0:
        t = a // b
        a, b = b, a - t * b
        k0, k1 = k1, t * k1 + k0
        if k1 >= 2 ** 53:
            break
        dens.append(k1)
    for k in dens:
        if k == 0:
            continue
        for mult in range(1, 64):
            if 2 ** 52 <= k * mult < 2 ** 53:
                xs.add(math.ldexp(float(k * mult), e))
        t = -(-(2 ** 52) // k)
        for mult in range(t, t + 6):
            if 2 ** 52 <= k * mult < 2 ** 53:
                xs.add(math.ldexp(float(k * mult), e))
mp.prec = 400
for k in list(range(1, 3000)) + [10 ** 5, 10 ** 6, 333333, 333334]:
    x = float(mpf(k) * pi / 2)
    for d in range(-3, 4):
        y = x
        for _ in range(abs(d)):
            y = math.nextafter(y, math.inf if d > 0 else -math.inf)
        if 0 < y < 2 ** 19:
            xs.add(y)
ranked = sorted(xs, key=lambda x: min(abs(sin(mpf(x))), abs(cos(mpf(x)))))[:KEEP]
out = bytearray()
for x in sorted(ranked):
    for v in (x, float(sin(mpf(x))), float(cos(mpf(x))), float(tan(mpf(x)))):
        out += struct.pack("<d", v)
with open(sys.argv[1] + "/cw_hard.bin", "wb") as f:
    f.write(out)
