"""Run the shared synthetic CIFP vector through the exact caller-selected native library."""
from __future__ import annotations

import ctypes
import sys
from dataclasses import FrozenInstanceError, is_dataclass
from pathlib import Path
from typing import get_type_hints

import openatca
from openatca import (CifpAirport, CifpBindings, CifpDiagnostic, CifpError,
    CifpLeg, CifpRow, CifpText, _CifpBuffers, _CifpCounts, _CifpError,
    _ErrorInfo, _OaCifpAirport)


def unhex(text: str) -> bytes:
    return bytes.fromhex(text)


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: cifp_ctypes_vectors.py LIBRARY FIXTURE_TSV")
    library_path, fixture_path = Path(sys.argv[1]), Path(sys.argv[2])
    if not library_path.is_absolute():
        raise SystemExit("library path must be absolute")
    rows = [line.split("\t") for line in fixture_path.read_text(encoding="ascii").splitlines()]
    assert rows[0] == ["case", "hex", "status", "kind", "line", "first_column",
                       "last_column", "limit", "actual", "previous", "found", "code", "flags"]
    kind, encoded = rows[1][:2]
    assert kind == "valid"
    data = unhex(encoded)
    valid_diagnostics = tuple(map(int, rows[1][2:]))
    assert valid_diagnostics[0] == 6

    public_types = [
        CifpAirport, CifpLeg, CifpDiagnostic,
        openatca.CifpMeta, openatca.CifpPosition, openatca.CifpAltitude,
        openatca.CifpFixRef, openatca.CifpRunway, openatca.CifpProcedure,
    ]
    assert all(name in openatca.__all__ for name in (t.__name__ for t in public_types))
    assert CifpRow is not object and CifpText is openatca.CifpText
    for row_type in public_types:
        assert is_dataclass(row_type)
        hints = get_type_hints(row_type)
        assert hints and all(annotation is not object for annotation in hints.values())
    result_hints = get_type_hints(openatca.CifpResult)
    assert result_hints["airports"] == tuple[CifpAirport, ...]
    assert result_hints["legs"] == tuple[CifpLeg, ...]
    library = ctypes.CDLL(str(library_path))
    parser = CifpBindings(library)
    result = parser.parse(data, source="synthetic.cifp")
    counts = result.counts
    assert counts.airports == counts.header_records == counts.cycles == 1
    assert (counts.runways, counts.localizers, counts.vhf_navaids, counts.ndbs) == (1, 1, 1, 2)
    assert (counts.waypoints, counts.moras, counts.airspaces, counts.boundaries) == (2, 1, 1, 1)
    assert (counts.airways, counts.airway_fixes) == (1, 1)
    assert counts.sections == 14
    assert counts.procedures == 3 and counts.routes == 3 and counts.legs == 3
    assert counts.continuations == 1 and counts.levels_of_service == 3
    assert counts.errors == 0 and result.clean and counts.is_clean
    airport = result.airports[0]
    assert is_dataclass(airport)
    try:
        airport.ident = None
    except FrozenInstanceError:
        pass
    else:
        raise AssertionError("public CIFP row type is mutable")
    assert airport.ident.text == "KZZZ" and airport.ident.offset < counts.strings_bytes
    assert airport.meta.line == 2 and airport.position.lat == 0
    assert result.runways[0].length_ft == 100
    assert result.runways[0].ident.text == "RW001"
    assert result.localizers[0].frequency_mhz == 111.75
    assert result.localizers[0].ident.text == "ISYN"
    assert result.vhf_navaids[0].frequency_mhz == 113.0
    assert result.vhf_navaids[0].name.text == "SYNTHETIC VOR"
    assert result.ndbs[0].frequency_khz == 365.0
    assert result.ndbs[0].ident.text == "SNDB"
    assert result.ndbs[1].terminal == 1
    assert result.waypoints[0].kind == 1
    assert result.waypoints[1].kind == 2
    assert result.moras[0].cells_ft[0] == 10000
    assert result.airspaces[0].kind == 1 and result.airspaces[0].boundary_count == 1
    assert result.airspaces[0].designation.text == "SYN01"
    assert result.boundaries[0].via == ord("G")
    assert result.airways[0].fix_count == 1
    assert result.airway_fixes[0].route_ident.text == "V001"
    assert result.airway_fixes[0].sequence == 1
    header = result.header[0]
    assert header.record_length == 132 and header.cycle_year == 26
    assert header.file_name.text == "SYNTHETIC-CIFP"
    procedures = result.procedures
    assert procedures[0].kind == 1 and procedures[1].kind == 2 and procedures[2].kind == 3
    assert procedures[0].route_count == 1
    assert result.legs[0].sequence == 10
    assert result.continuations[0].application_type == ord("W")
    assert result.levels_of_service[0].authorized == 1
    assert result.levels_of_service[1].authorized == 0
    assert result.sections[0].count == 1 and result.cycles[0].year == 26

    input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data)
    source = b"synthetic.cifp"
    source_buffer = (ctypes.c_uint8 * len(source)).from_buffer_copy(source)
    counts_out, detail, generic = _CifpCounts(), _CifpError(), _ErrorInfo()
    short_airport = (_OaCifpAirport * 1)()
    buffers = _CifpBuffers()
    buffers.airports = short_airport
    buffers.airports_capacity = 0
    status = int(library.oa_cifp_parse(input_buffer, len(data), source_buffer, len(source),
        ctypes.byref(buffers), ctypes.byref(counts_out), ctypes.byref(detail), ctypes.byref(generic)))
    assert status == 6 and counts_out.airports == 1
    valid_actual = (detail.status, detail.kind, detail.line, detail.first_column,
                    detail.last_column, detail.limit, detail.actual, detail.previous,
                    detail.found, detail.code, detail.flags)
    assert valid_actual == valid_diagnostics
    assert bytes(short_airport) == bytes(192)

    bad_kind, bad_encoded = rows[2][:2]
    bad_diagnostics = tuple(map(int, rows[2][2:]))
    assert bad_kind == "missing_header"
    try:
        parser.parse(unhex(bad_encoded), source="synthetic.cifp")
    except CifpError as error:
        assert error.code == error.status == bad_diagnostics[0] == 12
        actual = (error.status, error.kind, error.line, error.first_column,
                  error.last_column, error.limit, error.actual, error.previous,
                  error.found, error.diagnostic_code, error.flags)
        assert actual == bad_diagnostics
    else:
        raise AssertionError("malformed synthetic CIFP vector was accepted")
    print("ctypes CIFP vectors: all typed CIFP tables, capacity and malformed refusal")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
