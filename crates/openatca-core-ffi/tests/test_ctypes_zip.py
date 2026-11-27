import ctypes
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))
from openatca import OpenAtcaError, _ErrorInfo, _ZipEntry, load

if len(sys.argv) != 3:
    raise SystemExit("usage: test_ctypes_zip.py /absolute/path/to/library zip-v1.tsv")

api = load(Path(sys.argv[1]).resolve()).archives
fixture = Path(sys.argv[2])
assert ctypes.sizeof(_ZipEntry) == 56 and ctypes.alignment(_ZipEntry) == 8

for line in fixture.read_text(encoding="ascii").splitlines():
    if not line or line.startswith("#"):
        continue
    fields = line.split("\t")
    assert len(fields) == 8
    name, archive_hex, list_status, names_hex, index_text, extract_status, data_hex, declared_text = fields
    archive = bytes.fromhex(archive_hex)
    expected_list = int(list_status)
    if expected_list == 0:
        result = api.parse_zip(archive)
        assert len(result.entries) == 1, name
        item = result.entries[0]
        assert item.name.encode("utf-8") == bytes.fromhex(names_hex), name
        assert item.utf8_name_flag == bool(item.flags & (1 << 11)), name
        assert item.method in (0, 8), name
    else:
        try:
            api.parse_zip(archive)
        except OpenAtcaError as exc:
            assert exc.code == expected_list, (name, exc.code, expected_list)
        else:
            raise AssertionError(f"{name}: parser accepted a refused ZIP")

    if index_text == "-":
        continue
    index, expected_extract, declared = int(index_text), int(extract_status), int(declared_text)
    output = (ctypes.c_uint8 * max(declared, 1))(*([0xA5] * max(declared, 1)))
    output_length, error = ctypes.c_uint64(0xFFFFFFFFFFFFFFFF), _ErrorInfo()
    input_buffer = (ctypes.c_uint8 * len(archive)).from_buffer_copy(archive)
    status = int(api._library.oa_zip_extract_entry(
        input_buffer, len(archive), index, None, 0,
        ctypes.byref(output_length), ctypes.byref(error)))
    assert status == (0 if declared == 0 else 6) and output_length.value == declared, name
    before = bytes(output)
    status = int(api._library.oa_zip_extract_entry(
        input_buffer, len(archive), index, output if declared else None, declared,
        ctypes.byref(output_length), ctypes.byref(error)))
    assert status == expected_extract, (name, status, expected_extract)
    if expected_extract == 0:
        expected = bytes.fromhex(data_hex)
        assert output_length.value == len(expected) and bytes(output)[:len(expected)] == expected, name
        assert api.extract_zip_entry(archive, index) == expected, name
    else:
        assert bytes(output) == before and output_length.value == declared, name

# Short metadata buffers report exact sizes while preserving every caller byte.
stored = bytes.fromhex(next(line.split("\t")[1] for line in fixture.read_text(encoding="ascii").splitlines()
                            if line.startswith("stored\t")))
input_buffer = (ctypes.c_uint8 * len(stored)).from_buffer_copy(stored)
entry = _ZipEntry()
entry.name_offset = 77
pool = (ctypes.c_uint8 * 2)(0xA5, 0xA5)
count, names_length, error = ctypes.c_uint64(99), ctypes.c_uint64(99), _ErrorInfo()
status = int(api._library.oa_zip_list(input_buffer, len(stored), ctypes.byref(entry), 1,
    ctypes.byref(count), pool, 2, ctypes.byref(names_length), ctypes.byref(error)))
assert status == 6 and count.value == 1 and names_length.value == 5
assert entry.name_offset == 77 and bytes(pool) == b"\xA5\xA5"

output = (ctypes.c_uint8 * 2)(0xA5, 0xA5)
output_length = ctypes.c_uint64(99)
status = int(api._library.oa_zip_extract_entry(input_buffer, len(stored), 0, output, 2,
    ctypes.byref(output_length), ctypes.byref(error)))
assert status == 6 and output_length.value == 3 and bytes(output) == b"\xA5\xA5"
output_length.value = 99
status = int(api._library.oa_zip_extract_entry(input_buffer, len(stored), 1, output, 2,
    ctypes.byref(output_length), ctypes.byref(error)))
assert status == 40 and output_length.value == 99 and bytes(output) == b"\xA5\xA5"
