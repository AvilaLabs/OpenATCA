"""Shared L0 math, integrity and geodesy vectors for the provisional ctypes boundary."""

from __future__ import annotations

import csv
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "python"))

from openatca import load  # noqa: E402


def _args(value: str) -> list[float]:
    return [] if not value else [float(part) for part in value.split(";")]


def run(library_path: str) -> None:
    geo = load(library_path)
    vectors = Path(__file__).with_name("fixtures") / "l0.csv"
    with vectors.open(newline="", encoding="ascii") as source:
        for row in csv.DictReader(source):
            operation = row["operation"]
            if operation == "sha256":
                content = bytes.fromhex(row["args"]) if row["args"] else b""
                digest = geo.integrity.sha256(content)
                assert geo.integrity.sha256_hex(digest) == row["digest_hex"], row["case"]
                continue
            args = _args(row["args"])
            if operation == "math_unary":
                actual = (geo.math._unary(int(args[0]), args[1]),)
            elif operation == "math_binary":
                actual = (geo.math._binary(int(args[0]), args[1], args[2]),)
            elif operation == "math_ternary":
                actual = (geo.math.clamp(args[1], args[2], args[3]),)
            elif operation == "math_powi":
                actual = (geo.math.powi(args[0], int(args[1])),)
            elif operation == "math_sin_cos":
                actual = geo.math.sin_cos(args[0])
            elif operation == "math_compare":
                actual = (float(geo.math.compare(args[0], args[1])),)
            elif operation == "math_sign_negative":
                actual = (float(geo.math.is_sign_negative(args[0])),)
            elif operation == "math_f32_unary":
                actual = (geo.math._f32_unary(int(args[0]), args[1]),)
            elif operation == "math_f32_binary":
                actual = (geo.math._f32_binary(int(args[0]), args[1], args[2]),)
            elif operation == "math_f32_ternary":
                actual = (geo.math.clamp_f32(args[1], args[2], args[3]),)
            elif operation == "math_f32_compare":
                actual = (float(geo.math.compare_f32(args[0], args[1])),)
            elif operation == "math_f32_sign_negative":
                actual = (float(geo.math.is_sign_negative_f32(args[0])),)
            elif operation == "rhumb_inverse":
                result = geo.rhumb_inverse(*args)
                actual = (result.distance_m, result.course_deg)
            elif operation == "rhumb_direct":
                actual = geo.rhumb_direct(*args)
            elif operation == "rhumb_fraction":
                actual = geo.rhumb_point_at_fraction(*args)
            elif operation == "separation":
                result = geo.geodesic_rhumb_separation(*args[:4], int(args[4]))
                actual = (result.max_m, result.at_fraction, result.geodesic_m, result.rhumb_m)
            else:
                raise AssertionError(f"unknown vector operation: {operation}")
            expected = [float(part) for part in row["out1"].split(";")]
            if row["out2"]:
                expected.append(float(row["out2"]))
            tolerance = float(row["tolerance"])
            if row["case"] == "equator-zero-separation":
                assert math.isfinite(actual[1]) and 0.0 <= actual[1] <= 1.0, row["case"]
                actual = (actual[0], actual[2], actual[3])
            assert len(actual) == len(expected), row["case"]
            for got, want in zip(actual, expected):
                assert math.isfinite(got), row["case"]
                assert abs(got - want) <= tolerance, (row["case"], got, want)

    polygon_vectors = Path(__file__).with_name("fixtures") / "polygon.csv"
    with polygon_vectors.open(newline="", encoding="ascii") as source:
        for row in csv.DictReader(source):
            vertices = [tuple(map(float, pair.split("|"))) for pair in row["vertices"].split(";")]
            with geo.polygon(vertices) as polygon:
                assert polygon.edge_count == len(vertices), row["case"]
                actual = polygon.contains(float(row["latitude"]), float(row["longitude"]))
                assert actual is (row["inside"] == "1"), row["case"]


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: test_ctypes_l0.py /absolute/path/to/library")
    run(sys.argv[1])
