"""Descriptor-verified I/O for the Chicago comparison package scripts.

Every package read uses the sealed guard to check the requested path and verify the opened
descriptor before reading from it.
"""
from __future__ import annotations

import hashlib
import lzma
import os
import re
import stat
import sys
from pathlib import Path

# The wrapper invokes Python in isolated mode, which omits the script directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard

# Offline reproduction caps, chosen for whole-day packages on a workstation without allowing a
# manifest or decompressor to turn a review command into an unbounded read/allocation.
MAX_MANIFEST_BYTES = 64 * 1024
MAX_REPORT_BYTES = 512 * 1024 * 1024
MAX_EXPORT_BYTES = 512 * 1024 * 1024
MAX_XZ_MEMORY_BYTES = 128 * 1024 * 1024
MAX_XZ_OUTPUT_CHUNK = 1024 * 1024
MAX_XZ_INPUT_CHUNK = 64 * 1024
MAX_ARCHIVE_PARTS = 16
MAX_PART_BYTES = 8 * 1024 * 1024 * 1024
MAX_TOTAL_ARCHIVE_BYTES = 64 * 1024 * 1024 * 1024


def _validate_manifest_bytes(raw: bytes):
    if len(raw) > MAX_MANIFEST_BYTES:
        raise ValueError("MANIFEST exceeds 64 KiB")
    try:
        text = raw.decode("ascii")
    except UnicodeDecodeError as e:
        raise ValueError("MANIFEST must be ASCII") from e
    lines = text.splitlines()
    if any((ord(ch) < 32 and ch != "\n") or ord(ch) == 127 for ch in text):
        raise ValueError("MANIFEST contains a control character")
    if len(lines) > 512 or not lines or lines[0] != "reproduce-zau-manifest 1":
        raise ValueError("MANIFEST exceeds 512 lines or has an invalid header")
    scalar = {"day", "commit", "label", "clip-args", "clip", "eram-export", "eram-export-xz", "adsb-release", "adsb-url"}
    seen = set()
    parts = 0
    total = 0
    part_names = set()
    outputs = set()
    named_files = {}
    reserved = {"ledger.txt", "p002.stdout", "p002.err", "clip.out", "clip.err", "build.log", "out", "MANIFEST"}
    allowed = scalar | {"adsb-release", "adsb-url", "adsb-file", "output"}
    for number, line in enumerate(lines[1:], 2):
        if len(line) > 16 * 1024:
            raise ValueError(f"MANIFEST line {number} exceeds 16 KiB")
        if not line or line.startswith("#"):
            continue
        key, sep, value = line.partition(" ")
        if not sep or key not in allowed or not value:
            raise ValueError(f"MANIFEST line {number} has an unknown or empty field")
        if key in scalar:
            if key in seen:
                raise ValueError(f"MANIFEST repeats scalar field {key}")
            seen.add(key)
        if key in {"clip", "eram-export", "eram-export-xz"}:
            fields = value.split()
            if len(fields) != 3 or not fields[1].isdecimal() or not re.fullmatch(r"[0-9a-f]{64}", fields[2]):
                raise ValueError(f"MANIFEST {key} needs exactly name, byte size and SHA-256")
            filename = fields[0]
            if not re.fullmatch(r"[A-Za-z0-9._+-]+", filename) or filename in {".", ".."} or filename in reserved:
                raise ValueError(f"MANIFEST {key} uses an unsafe or reserved output name")
            named_files[key] = filename
        if key == "adsb-file":
            parts += 1
            fields = value.split()
            if len(fields) != 4 or not re.fullmatch(r"[0-9a-f]{64}", fields[2]):
                raise ValueError("adsb-file needs exactly name, size, SHA-256 and URL")
            try:
                size = int(fields[1])
            except ValueError as e:
                raise ValueError("archive part size is not an integer") from e
            if size < 0 or size > MAX_PART_BYTES:
                raise ValueError("archive part exceeds 8 GiB")
            if fields[0] in part_names:
                raise ValueError("MANIFEST repeats an archive part name")
            if not re.fullmatch(r"[A-Za-z0-9._+-]+", fields[0]) or fields[0] in {".", ".."}:
                raise ValueError("archive part name must be a safe basename")
            part_names.add(fields[0])
            total += size
            if total > MAX_TOTAL_ARCHIVE_BYTES:
                raise ValueError("archive parts exceed 64 GiB total")
        if key == "clip-args":
            args = value.split()
            i = 0
            options = {"--box", "--from", "--to", "--frame-ms"}
            used = set()
            while i < len(args):
                option = args[i]
                if option not in options or option in used or i + 1 >= len(args):
                    raise ValueError("clip-args contains an unsupported, repeated or valueless option")
                value = args[i + 1]
                if value.startswith("-"):
                    raise ValueError("clip-args option value is not allowed")
                if option == "--frame-ms" and (not value.isdecimal() or not 1 <= int(value) <= 60000):
                    raise ValueError("clip-args frame-ms must be 1..60000")
                if option == "--box":
                    fields = value.split(",")
                    try:
                        nums = [float(v) for v in fields]
                    except ValueError as e:
                        raise ValueError("clip-args box must contain four numbers") from e
                    if len(nums) != 4 or not all(map(lambda n: abs(n) <= 180 and n == n, nums)):
                        raise ValueError("clip-args box must contain four bounded coordinates")
                if option in {"--from", "--to"} and not (
                    re.fullmatch(r"[0-9]{1,12}", value)
                    or re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z", value)
                ):
                    raise ValueError("clip-args time must be epoch seconds or compact ISO UTC")
                used.add(option)
                i += 2
        if key == "output":
            fields = value.split()
            if len(fields) not in {2, 3} or any(not re.fullmatch(r"[0-9a-f]{64}", h) for h in fields[1:]):
                raise ValueError("output needs a name and its expected SHA-256 value(s)")
            name = fields[0]
            if name not in {"p002.txt", "samples.csv"} or name in outputs:
                raise ValueError("MANIFEST has an unknown or repeated output")
            if (name == "p002.txt" and len(fields) != 3) or (name == "samples.csv" and len(fields) != 2):
                raise ValueError("MANIFEST output digest count is invalid")
            outputs.add(name)
    required = scalar - {"eram-export-xz", "adsb-url"}
    if not required <= seen or not 1 <= parts <= MAX_ARCHIVE_PARTS or outputs != {"p002.txt", "samples.csv"}:
        raise ValueError("MANIFEST is missing required fields or has an invalid archive-part count")
    if named_files.get("eram-export-xz") not in (None, named_files.get("eram-export", "") + ".xz"):
        raise ValueError("compressed export name must be the export name plus .xz")
    all_names = list(named_files.values()) + list(part_names)
    if "eram-export" in named_files:
        all_names.append(named_files["eram-export"] + ".sha256")
    if len(set(all_names)) != len(all_names):
        raise ValueError("MANIFEST reuses an archive, export, sidecar or clip name")
    values = {}
    for line in lines[1:]:
        if line and not line.startswith("#"):
            key, _, value = line.partition(" ")
            if key in scalar:
                values[key] = value
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", values.get("day", "")):
        raise ValueError("MANIFEST day must be YYYY-MM-DD")
    if not re.fullmatch(r"[0-9a-f]{40}", values.get("commit", "")):
        raise ValueError("MANIFEST commit must be 40 lower-case hexadecimal characters")
    if not values.get("label", "").strip():
        raise ValueError("MANIFEST label is empty")


def _validated_manifest(path: str) -> bytes:
    with _read(path) as source:
        raw = source.read(MAX_MANIFEST_BYTES + 1)
    _validate_manifest_bytes(raw)
    return raw


def _validate_writer_fields(label: str, clip_args: str, names: list[str]):
    for field, value in (("label", label), ("clip-args", clip_args)):
        try:
            value.encode("ascii")
        except UnicodeEncodeError as e:
            raise ValueError(f"{field} must be ASCII") from e
        if len(value) > 16 * 1024 or any(ord(ch) < 32 or ord(ch) > 126 for ch in value):
            raise ValueError(f"{field} must be a single printable ASCII line")
    if not label.strip():
        raise ValueError("label is empty")
    if not names or len(names) > MAX_ARCHIVE_PARTS + 2:
        raise ValueError("invalid number of package basenames")
    export = names[0]
    if os.path.basename(export) != export:
        raise ValueError("export name must be a basename")
    role_paths = names[1:]
    if not role_paths or os.path.basename(role_paths[0]) in {"", ".", ".."}:
        raise ValueError("clip path has no safe basename")
    names = [export, os.path.basename(role_paths[0]), *(os.path.basename(p) for p in role_paths[1:])]
    for name in names:
        if not re.fullmatch(r"[A-Za-z0-9._+-]+", name) or name in {".", ".."}:
            raise ValueError(f"unsafe package basename: {name!r}")
    export, clip, *parts = names
    role_names = [export, export + ".sha256", export + ".xz", clip, *parts]
    reserved = {"ledger.txt", "p002.stdout", "p002.err", "clip.out", "clip.err", "build.log", "out", "MANIFEST"}
    if any(name in reserved for name in role_names):
        raise ValueError("package basename aliases a wrapper output")
    if len(set(role_names)) != len(role_names):
        raise ValueError("package basenames collide across export, sidecar, compressed export, clip or parts")


def _read(path: str):
    guard = sealed_guard.production()
    guard.check(path)
    # Opening a FIFO for reading must not wait for a writer.
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        guard.verify_fd(fd, path)
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError(f"{path}: expected a regular file")
        source = os.fdopen(fd, "rb")
    except BaseException:
        os.close(fd)
        raise
    return source


def _measure(path: str, cap: int = MAX_REPORT_BYTES):
    digest = hashlib.sha256()
    size = 0
    with _read(path) as source:
        while True:
            block = source.read(1024 * 1024)
            if not block:
                break
            size += len(block)
            if size > cap:
                raise ValueError(f"{path}: exceeds the {cap}-byte read limit")
            digest.update(block)
    return size, digest.hexdigest()


def _unpack(source_path: str, out_path: str, expected_size: int, expected_sha: str):
    if expected_size < 0 or expected_size > MAX_EXPORT_BYTES:
        raise ValueError("declared export size exceeds 512 MiB")
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha):
        raise ValueError("declared export SHA-256 is malformed")
    guard = sealed_guard.production()
    # Open and verify the compressed bytes once; decompression consumes that descriptor.
    with _read(source_path) as source:
        if os.fstat(source.fileno()).st_size > MAX_PART_BYTES:
            raise ValueError("compressed input exceeds 8 GiB")
        guard.check(out_path)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(out_path, flags, 0o600)
        opened_stat = None
        try:
            opened_stat = os.fstat(fd)
            guard.verify_fd(fd, out_path)
            digest = hashlib.sha256()
            size = 0
            with os.fdopen(fd, "wb", closefd=True) as output:
                fd = -1
                decoded = lzma.LZMADecompressor(
                    format=lzma.FORMAT_XZ, memlimit=MAX_XZ_MEMORY_BYTES
                )
                compressed = 0
                while not decoded.eof:
                    if decoded.needs_input:
                        source_block = source.read(MAX_XZ_INPUT_CHUNK)
                        if not source_block:
                            raise ValueError("truncated XZ stream")
                        compressed += len(source_block)
                        if compressed > MAX_PART_BYTES:
                            raise ValueError("compressed input exceeds 8 GiB")
                    else:
                        source_block = b""
                    block = decoded.decompress(source_block, max_length=MAX_XZ_OUTPUT_CHUNK)
                    size += len(block)
                    if size > min(expected_size, MAX_EXPORT_BYTES):
                        raise ValueError("unpacked size exceeds the manifest or 512 MiB safety limit")
                    output.write(block)
                    digest.update(block)
                if decoded.unused_data or source.read(1):
                    raise ValueError("trailing bytes or a second member after the single XZ stream")
                output.flush()
                os.fsync(output.fileno())
            got = digest.hexdigest()
            if size != expected_size:
                raise ValueError(f"unpacked {size} bytes, manifest says {expected_size}")
            if got != expected_sha:
                raise ValueError(f"unpacked SHA-256 {got} does not match the manifest's {expected_sha}")
        except BaseException:
            if fd >= 0:
                os.close(fd)
            if opened_stat is not None:
                try:
                    current = os.stat(out_path, follow_symlinks=False)
                    if (current.st_dev, current.st_ino) == (opened_stat.st_dev, opened_stat.st_ino):
                        os.unlink(out_path)
                except OSError:
                    pass
            raise


def main(argv: list[str]) -> int:
    try:
        if len(argv) >= 5 and argv[0] == "validate-writer-fields":
            _validate_writer_fields(argv[1], argv[2], argv[3:])
            return 0
        if len(argv) == 2 and argv[0] == "validate-manifest":
            _validated_manifest(argv[1])
            return 0
        if len(argv) == 2 and argv[0] == "read-manifest":
            sys.stdout.buffer.write(_validated_manifest(argv[1]))
            return 0
        if len(argv) == 2 and argv[0] == "read":
            with _read(argv[1]) as source:
                limit = MAX_MANIFEST_BYTES if Path(argv[1]).name == "MANIFEST" else MAX_REPORT_BYTES
                copied = 0
                while True:
                    block = source.read(min(64 * 1024, limit + 1 - copied))
                    if not block:
                        break
                    copied += len(block)
                    if copied > limit:
                        raise ValueError(f"{argv[1]} exceeds the {limit}-byte read limit")
                    sys.stdout.buffer.write(block)
            return 0
        if len(argv) == 2 and argv[0] == "present":
            with _read(argv[1]):
                pass
            return 0
        if len(argv) == 2 and argv[0] == "measure":
            cap = MAX_PART_BYTES if ".tar." in Path(argv[1]).name or Path(argv[1]).suffix == ".xz" else MAX_REPORT_BYTES
            size, digest = _measure(argv[1], cap)
            print(size, digest)
            return 0
        if len(argv) == 4 and argv[0] == "check":
            path, want_size, want_sha = argv[1:]
            try:
                expected_size = int(want_size)
            except ValueError as e:
                raise ValueError("manifest size is not an integer") from e
            cap = MAX_PART_BYTES if ".tar." in Path(path).name or Path(path).suffix == ".xz" else MAX_EXPORT_BYTES
            size, got = _measure(path, cap)
            if size != expected_size:
                raise ValueError(
                    f"{Path(path).name} has size {size}, the MANIFEST says {expected_size}"
                )
            if got != want_sha:
                raise ValueError(f"{path}: SHA-256 {got} does not match the MANIFEST's {want_sha}")
            return 0
        if len(argv) == 2 and argv[0] == "masked-hash":
            with _read(argv[1]) as source:
                text = source.read(MAX_REPORT_BYTES + 1)
            if len(text) > MAX_REPORT_BYTES:
                raise ValueError("report exceeds the 512 MiB read limit")
            text = re.sub(rb"(scorer binary sha256: )[0-9a-f]{64}", rb"\1BINARY", text)
            text = re.sub(rb"( binary=)[0-9a-f]{64}", rb"\1BINARY", text)
            print(hashlib.sha256(text).hexdigest())
            return 0
        if len(argv) == 5 and argv[0] == "unpack-xz":
            _, source, output, size, digest = argv
            _unpack(source, output, int(size), digest)
            return 0
        print(
            "usage: repro_package_io.py validate-writer-fields LABEL CLIP_ARGS EXPORT CLIP PART...; "
            "validate-manifest|read-manifest|read|present|measure|masked-hash PATH; "
            "check PATH SIZE SHA256; unpack-xz INPUT OUTPUT SIZE SHA256",
            file=sys.stderr,
        )
        return 2
    except FileNotFoundError as e:
        if argv[:1] != ["present"]:
            print(f"repro_package_io: {e}", file=sys.stderr)
        return 3
    except (sealed_guard.SealedError, OSError, ValueError, lzma.LZMAError) as e:
        print(f"repro_package_io: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
