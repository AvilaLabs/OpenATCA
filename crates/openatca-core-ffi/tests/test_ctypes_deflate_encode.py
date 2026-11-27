import ctypes
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))
from openatca import load

library = Path(sys.argv[1]).resolve()
fixture = Path(sys.argv[2])
api = load(library)

class ErrorRecord(ctypes.Structure):
    _fields_ = [("code", ctypes.c_uint32), ("location_kind", ctypes.c_uint32), ("location", ctypes.c_uint64)]

for line in fixture.read_text(encoding="utf-8").splitlines():
    if not line or line.startswith("#"):
        continue
    name, input_hex, level, max_output, expected_status, capacity, expected_consumed, expected_level, plain_hex = line.split("\t")
    source = bytes.fromhex(input_hex)
    plain = bytes.fromhex(plain_hex)
    if int(expected_status) == 0:
        result = api.compression.raw_deflate(source, level=int(level), max_output=int(max_output))
        assert result.consumed == int(expected_consumed), name
        assert result.actual_level == int(expected_level), name
        decoded = api.compression.raw_inflate(result.output, max_output=len(plain))
        assert decoded.output == plain, name
        assert decoded.consumed == len(result.output), name
        continue

    input_buffer = (ctypes.c_uint8 * len(source)).from_buffer_copy(source) if source else None
    output = (ctypes.c_uint8 * int(capacity))() if int(capacity) else None
    if output is not None:
        ctypes.memset(output, 0xA5, int(capacity))
    output_length = ctypes.c_uint64(777)
    consumed = ctypes.c_uint64(777)
    actual_level = ctypes.c_uint32(777)
    error = ErrorRecord()
    status = int(api._library.oa_deflate_raw_compress(
        input_buffer, len(source), int(level), int(max_output), output, int(capacity),
        ctypes.byref(output_length), ctypes.byref(consumed), ctypes.byref(actual_level),
        ctypes.byref(error)))
    assert status == int(expected_status), (name, status)
    if status == 6:
        assert output_length.value > 0 and consumed.value == int(expected_consumed), name
        assert actual_level.value == int(expected_level), name
        assert bytes(output) == bytes([0xA5]) * int(capacity), name
        required = output_length.value
        exact_output = (ctypes.c_uint8 * required)()
        exact_length = ctypes.c_uint64(777)
        exact_consumed = ctypes.c_uint64(777)
        exact_level = ctypes.c_uint32(777)
        status = int(api._library.oa_deflate_raw_compress(
            input_buffer, len(source), int(level), int(max_output), exact_output, required,
            ctypes.byref(exact_length), ctypes.byref(exact_consumed), ctypes.byref(exact_level),
            ctypes.byref(error)))
        assert status == 0 and exact_length.value == required, name
        assert exact_consumed.value == int(expected_consumed), name
        assert exact_level.value == int(expected_level), name
        decoded = api.compression.raw_inflate(bytes(exact_output), max_output=len(plain))
        assert decoded.output == plain, name
        assert decoded.consumed == required, name
    else:
        assert output_length.value == 777 and consumed.value == 777 and actual_level.value == 777, name
        if output is not None:
            assert bytes(output) == bytes([0xA5]) * int(capacity), name
