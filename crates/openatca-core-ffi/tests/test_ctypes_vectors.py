"""Run the shared synthetic geodesy vectors through the provisional ctypes wrapper.

This source is not run at import time. The root coordinator supplies the built library and invokes
it explicitly after the source and ABI gates pass.
"""

from __future__ import annotations

import csv
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "python"))

from openatca import OpenAtcaError, load  # noqa: E402


def run(library_path: str, vectors_path: str | None = None) -> None:
    path = Path(vectors_path) if vectors_path else Path(__file__).with_name("fixtures") / "wgs84.csv"
    geo = load(library_path)
    with path.open(newline="", encoding="ascii") as source:
        for row in csv.DictReader(source):
            args = [float(row[f"arg{i}"]) for i in range(1, 5)]
            expected_status = int(row["status"])
            expected_location = int(row["location"])
            try:
                if row["operation"] == "inverse":
                    answer = geo.inverse(*args)
                    outputs = (answer.distance_m, answer.azimuth1_deg, answer.azimuth2_deg)
                elif row["operation"] == "direct":
                    answer = geo.direct(*args)
                    outputs = (answer.lat2_deg, answer.lon2_deg, answer.azimuth2_deg)
                else:
                    raise AssertionError(f"unknown vector operation: {row['operation']}")
                status = 0
                location_kind = 0
                location = 0
            except OpenAtcaError as error:
                status = error.code
                location_kind = error.location_kind
                location = error.location
                outputs = ()
            assert status == expected_status, (row["case"], status, expected_status)
            if status == 0:
                assert location_kind == 0, row["case"]
                tolerance = float(row["tolerance"])
                for actual, expected in zip(outputs, (row["out1"], row["out2"], row["out3"])):
                    if expected:
                        assert math.isfinite(actual), row["case"]
                        assert abs(actual - float(expected)) <= tolerance, (
                            row["case"], actual, expected, tolerance
                        )
                    else:
                        assert math.isfinite(actual), row["case"]
            else:
                assert location_kind == 1, row["case"]
                assert location == expected_location, (row["case"], location, expected_location)


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        raise SystemExit("usage: test_ctypes_vectors.py /absolute/path/to/library [vectors.csv]")
    run(sys.argv[1], sys.argv[2] if len(sys.argv) == 3 else None)
