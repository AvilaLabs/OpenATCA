import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))

from openatca import OpenAtcaError, load

"""REQ: OA-FFI-001, OA-FFI-002, OA-FFI-005"""
library, vector = sys.argv[1:]
line = Path(vector).read_text(encoding="ascii").splitlines()[1]
data = bytes.fromhex(line.split("\t")[1])
import ctypes
lib = ctypes.CDLL(library)
api = load(Path(library).resolve()).grib2
assert api.inspect_message(data) == (1, 12)
message = api.decode_message(data)
assert len(message.fields) == 1
field = message.fields[0]
assert (field.discipline, field.category, field.number) == (0, 2, 2)
assert (field.nx, field.ny, field.missing_count) == (4, 3, 5)
assert field.grid_parameters[:4] == (42.0, -90.0, 0.25, 0.5)
assert field.reference_time == (2026, 11, 14, 18, 0, 0)
assert field.forecast_time == 6
assert message.values[0] == 1.0 and message.values[2] == 2.0
assert message.values[1] != message.values[1] and message.values[11] != message.values[11]
try:
    api.decode_message(data[:-1])
except OpenAtcaError:
    pass
else:
    raise AssertionError("truncated GRIB2 input was accepted")
sections = []
pos = 16
while pos + 5 <= len(data) - 4:
    length = int.from_bytes(data[pos:pos + 4], "big")
    sections.append((data[pos + 4], pos))
    pos += length
unsupported = bytearray(data)
section5 = next(offset for number, offset in sections if number == 5)
unsupported[section5 + 9:section5 + 11] = (41).to_bytes(2, "big")
try:
    api.decode_message(bytes(unsupported))
except OpenAtcaError as exc:
    assert exc.code == 81
else:
    raise AssertionError("unsupported packing template was accepted")
