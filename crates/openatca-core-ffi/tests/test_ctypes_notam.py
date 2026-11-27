"""ctypes C ABI checks over the shared synthetic FNS profile vector."""
from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "python"))
from openatca import NotamError, load  # noqa: E402

VECTOR = Path(__file__).resolve().parent / "fixtures" / "notam-v1.xml"
ISSUED_MS = 1_898_931_600_000


def run(library_path: str) -> int:
    api = load(library_path).notams
    data = VECTOR.read_bytes()
    result = api.decode(data, drop_window_ms=(0, 0))
    if result.row is None or result.row.year != 2030 or result.row.number != 42:
        raise AssertionError("NOTAM identity differs from the decoder vector")
    if not result.row.present & (1 << 12) or result.row.text_len == 0:
        raise AssertionError("NOTAM text/presence was not retained")
    if len(result.polygons) != 1 or len(result.vertices) != 4:
        raise AssertionError("polygon metadata/vertices differ from the synthetic profile vector")
    if (len(result.unknown_paths) != 1 or result.summary.unknown_elements != 1 or
            result.unknown_paths[0].count != 1 or
            not result.unknown_paths[0].text.endswith(b"event:novel")):
        raise AssertionError("unknown path record/count was not retained")
    if result.vertices[0].lat_e7 != 301_000_000 or result.vertices[0].lon_e7 != -901_000_000:
        raise AssertionError("E7 polygon coordinates changed across ctypes")
    retained = result.strings[result.row.text_off:
                               result.row.text_off + result.row.text_len]
    if retained != b"RWY 09/27 CLSD":
        raise AssertionError("row text offsets do not index the caller's UTF-8 arena")

    dropped = api.decode(VECTOR.read_bytes(), drop_window_ms=(ISSUED_MS, ISSUED_MS + 1))
    if dropped.row is not None or dropped.summary.sealed_dropped != 1:
        raise AssertionError("explicit half-open drop interval did not report its dropped row")
    adjacent = api.decode(VECTOR.read_bytes(), drop_window_ms=(ISSUED_MS + 1, ISSUED_MS + 2))
    if adjacent.row is None or adjacent.summary.sealed_dropped:
        raise AssertionError("half-open interval dropped a row at its exclusive end")

    bad_time = data.replace(b"<event:issued>2030-03-05T09:00:00.000Z",
                            b"<event:issued>bad", 1)
    damaged = api.decode(bad_time, drop_window_ms=(0, 0))
    if damaged.row is None or damaged.row.present & (1 << 8) or not damaged.row.errors & (1 << 8):
        raise AssertionError("invalid issue time lost its absent/error distinction")

    bad_polygon = data.replace(
        b"<gml:posList>30.10 -90.10 30.10 -90.20 30.20 -90.20 30.20 -90.10 30.10 -90.10</gml:posList>",
        b"<gml:posList>30.10 -90.10 999 999</gml:posList>", 1)
    damaged = api.decode(bad_polygon, drop_window_ms=(0, 0))
    if damaged.row is None or damaged.row.errors & (1 << 25) == 0 or damaged.polygons:
        raise AssertionError("bad polygon was not reported atomically as a field error")

    try:
        api.decode(data, drop_window_ms=(2, 1))
    except NotamError as error:
        if error.code != 1:
            raise AssertionError("inverted interval returned the wrong status") from error
    else:
        raise AssertionError("inverted interval was accepted")
    try:
        api.decode(data)
    except TypeError:
        pass
    else:
        raise AssertionError("decoder accepted an implicit drop-window policy")
    return 4


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: test_ctypes_notam.py /absolute/path/to/libopenatca_core_ffi.so")
    print(f"NOTAM ctypes vectors passed: {run(sys.argv[1])}")
