import ctypes
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))
from openatca import OpenAtcaError, _ErrorInfo, _TarEntry, load

library = Path(sys.argv[1]).resolve()
fixture = Path(sys.argv[2])
api = load(library)

for line in fixture.read_text(encoding="utf-8").splitlines():
    if not line or line.startswith("#"):
        continue
    name, archive_hex, max_entries, expected_status, entry_capacity, name_capacity, expected_count, names_hex = line.split("\t")
    archive = bytes.fromhex(archive_hex)
    expected_names = bytes.fromhex(names_hex)
    entry_capacity = int(entry_capacity)
    name_capacity = int(name_capacity)
    entries = (_TarEntry * entry_capacity)() if entry_capacity else None
    names = (ctypes.c_uint8 * name_capacity)() if name_capacity else None
    if entries is not None:
        ctypes.memset(entries, 0xA5, ctypes.sizeof(entries))
    if names is not None:
        ctypes.memset(names, 0xA5, name_capacity)
    initial_entries = bytes(entries) if entries is not None else b""
    initial_names = bytes(names) if names is not None else b""
    input_buffer = (ctypes.c_uint8 * len(archive)).from_buffer_copy(archive) if archive else None
    count, names_length, error = ctypes.c_uint64(777), ctypes.c_uint64(777), _ErrorInfo()
    status = int(api._library.oa_tar_parse(
        input_buffer, len(archive), int(max_entries), entries, entry_capacity,
        ctypes.byref(count), names, name_capacity, ctypes.byref(names_length), ctypes.byref(error)))
    assert status == int(expected_status), (name, status)
    if status == 0:
        assert count.value == int(expected_count) and names_length.value == len(expected_names), name
        if names_length.value:
            assert bytes(names)[:names_length.value] == expected_names, name
        if name == "valid_duplicate_unknown":
            assert entries[0].name_offset == 0 and entries[0].name_length == 4, name
            assert entries[0].size == 3 and entries[0].mtime == 0o14000000000, name
            assert entries[0].header_offset == 0 and entries[0].typeflag == ord("0"), name
            assert entries[1].name_offset == 4 and entries[1].name_length == 4, name
            assert entries[1].size == 0 and entries[1].mtime == 0o14000000000, name
            assert entries[1].header_offset == 1024 and entries[1].typeflag == ord("?"), name
            assert bytes(entries[0].reserved) == bytes(7) and bytes(entries[1].reserved) == bytes(7), name
        result = api.archives.parse_tar(archive, max_entries=int(max_entries))
        assert len(result.entries) == int(expected_count), name
        if name == "valid_duplicate_unknown":
            assert [entry.name for entry in result.entries] == ["same", "same", "café.txt"], name
            assert [entry.typeflag for entry in result.entries] == [ord("0"), ord("?"), ord("0")], name
    elif status == 6:
        assert count.value == int(expected_count) and names_length.value == len(expected_names), name
        assert (bytes(entries) if entries is not None else b"") == initial_entries, name
        assert (bytes(names) if names is not None else b"") == initial_names, name
        result = api.archives.parse_tar(archive, max_entries=int(max_entries))
        assert [entry.name for entry in result.entries] == ["same", "same", "café.txt"], name
    else:
        assert count.value == 777 and names_length.value == 777, name
        assert (bytes(entries) if entries is not None else b"") == initial_entries, name
        assert (bytes(names) if names is not None else b"") == initial_names, name
        if name == "cap_above_max":
            try:
                api.archives.parse_tar(archive, max_entries=int(max_entries))
            except ValueError:
                pass
            else:
                raise AssertionError("Python facade accepted a max_entries value above its cap")
        else:
            try:
                api.archives.parse_tar(archive, max_entries=int(max_entries))
            except OpenAtcaError as exc:
                assert exc.code == int(expected_status), (name, exc.code)
            else:
                raise AssertionError(f"{name}: Python facade accepted a refused archive")
