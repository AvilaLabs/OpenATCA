import ctypes
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))
from openatca import _ErrorInfo, load

library = Path(sys.argv[1]).resolve()
fixture = Path(sys.argv[2])
api = load(library)

for line in fixture.read_text(encoding="utf-8").splitlines():
    if not line or line.startswith("#"):
        continue
    name, encoded, limit, expected_status, capacity, output_hex, consumed = line.split("\t")
    source = bytes.fromhex(encoded)
    if int(expected_status) == 0:
        result = api.compression.raw_inflate(source, max_output=int(limit))
        assert result.output.hex() == output_hex, name
        assert result.consumed == int(consumed), name
    else:
        error = _ErrorInfo()
        out_length = ctypes.c_uint64(777)
        used = ctypes.c_uint64(777)
        buffer = (ctypes.c_uint8 * len(source)).from_buffer_copy(source) if source else None
        output = (ctypes.c_uint8 * int(capacity))() if int(capacity) else None
        status = int(api._library.oa_deflate_raw_inflate(
            buffer, len(source), int(limit), output, int(capacity),
            ctypes.byref(out_length), ctypes.byref(used), ctypes.byref(error)))
        assert status == int(expected_status), (name, status)
