"""Small stdlib-only PEP 517 wheel backend for the provisional ctypes package."""

from __future__ import annotations

import base64
import csv
import hashlib
import io
import os
from pathlib import Path
import posixpath
import stat
import zipfile


NAME = "openatca"
VERSION = "0.1.0"
DIST_INFO = "openatca-0.1.0.dist-info"
WHEEL_FILENAME = "openatca-0.1.0-py3-none-any.whl"
MAX_FILE_BYTES = 4 * 1024 * 1024
MAX_TOTAL_BYTES = 10 * 1024 * 1024
ZIP_TIME = (1980, 1, 1, 0, 0, 0)
FILES = (
    ("openatca/__init__.py", "openatca/__init__.py"),
    ("openatca/py.typed", "openatca/py.typed"),
    (f"{DIST_INFO}/licenses/LICENSE", "LICENSE"),
    (f"{DIST_INFO}/METADATA", None),
    (f"{DIST_INFO}/WHEEL", None),
)


def _open_directory_chain(path: Path) -> int:
    """Open every absolute path component without following symlinks."""
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("directory path must be absolute and contain no parent traversal")
    if (os.open not in os.supports_dir_fd or not hasattr(os, "O_NOFOLLOW") or
            not hasattr(os, "O_DIRECTORY") or not hasattr(os, "O_NONBLOCK")):
        raise ValueError("safe packaging requires directory-relative no-follow opens")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)
    fd = os.open(os.sep, flags)
    try:
        for component in path.parts[1:]:
            next_fd = os.open(component, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return fd
    except OSError as exc:
        os.close(fd)
        raise ValueError("directory path is missing, non-directory, or contains a symlink") from exc
    except Exception:
        os.close(fd)
        raise


def _source_root() -> Path:
    module_path = Path(__file__)
    if not module_path.is_absolute() or ".." in module_path.parts:
        raise ValueError("backend module path must be absolute without parent traversal")
    return module_path.parent


def _read_source(relative: str) -> bytes:
    root = _source_root()
    if (not relative or "\\" in relative or posixpath.isabs(relative) or
            any(part in ("", ".", "..") for part in relative.split("/"))):
        raise ValueError("package input path must be a clean relative POSIX path")
    parts = relative.split("/")
    directory_fd = _open_directory_chain(root)
    try:
        for component in parts[:-1]:
            next_fd = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                              dir_fd=directory_fd)
            os.close(directory_fd)
            directory_fd = next_fd
        fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK |
                     getattr(os, "O_CLOEXEC", 0),
                     dir_fd=directory_fd)
        try:
            opened = os.fstat(fd)
            if not stat.S_ISREG(opened.st_mode):
                raise ValueError(f"package input must be a regular file: {relative}")
            if opened.st_size > MAX_FILE_BYTES:
                raise ValueError(f"package input exceeds {MAX_FILE_BYTES} bytes: {relative}")
            with os.fdopen(fd, "rb", closefd=False) as stream:
                data = stream.read(MAX_FILE_BYTES + 1)
            after = os.fstat(fd)
            if (len(data) != opened.st_size or len(data) > MAX_FILE_BYTES or
                    (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns,
                     opened.st_ctime_ns) !=
                    (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns,
                     after.st_ctime_ns)):
                raise ValueError(f"package input changed or exceeded its limit: {relative}")
            return data
        finally:
            os.close(fd)
    finally:
        os.close(directory_fd)


def _metadata(readme: bytes) -> bytes:
    try:
        description = readme.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("README must be UTF-8") from exc
    fields = [
        "Metadata-Version: 2.4",
        f"Name: {NAME}",
        f"Version: {VERSION}",
        "Summary: Provisional pure-Python ctypes wrapper for selected OpenATCA L0 C ABI operations",
        "Requires-Python: >=3.10",
        "License-Expression: Apache-2.0",
        "License-File: LICENSE",
        "Author: Avila Labs",
        "Classifier: Development Status :: 2 - Pre-Alpha",
        "Classifier: License :: OSI Approved :: Apache Software License",
        "Classifier: Operating System :: OS Independent",
        "Classifier: Programming Language :: Python :: 3",
        "Classifier: Programming Language :: Python :: 3 :: 10",
        "Classifier: Programming Language :: Python :: 3 :: 11",
        "Classifier: Programming Language :: Python :: 3 :: 12",
        "Classifier: Programming Language :: Python :: 3 :: 13",
        "Classifier: Topic :: Scientific/Engineering",
        "Description-Content-Type: text/markdown",
        "",
        description.rstrip("\n"),
        "",
    ]
    return "\n".join(fields).encode("utf-8")


def _wheel() -> bytes:
    entries: dict[str, bytes] = {}
    total = 0
    for archive_name, source_name in FILES:
        if archive_name.endswith("/METADATA"):
            payload = _metadata(_read_source("README.md"))
        elif archive_name.endswith("/WHEEL"):
            payload = (
                "Wheel-Version: 1.0\n"
                "Generator: openatca-stdlib-wheel-backend\n"
                "Root-Is-Purelib: true\n"
                "Tag: py3-none-any\n"
            ).encode("ascii")
        else:
            assert source_name is not None
            payload = _read_source(source_name)
        if len(payload) > MAX_FILE_BYTES:
            raise ValueError(f"wheel member exceeds {MAX_FILE_BYTES} bytes: {archive_name}")
        total += len(payload)
        if total > MAX_TOTAL_BYTES:
            raise ValueError(f"wheel inputs exceed {MAX_TOTAL_BYTES} bytes")
        entries[archive_name] = payload

    record_path = f"{DIST_INFO}/RECORD"
    record = io.StringIO(newline="")
    writer = csv.writer(record, lineterminator="\n")
    for name, payload in sorted(entries.items()):
        digest = base64.urlsafe_b64encode(hashlib.sha256(payload).digest()).rstrip(b"=").decode("ascii")
        writer.writerow((name, f"sha256={digest}", str(len(payload))))
    writer.writerow((record_path, "", ""))
    entries[record_path] = record.getvalue().encode("utf-8")

    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, payload in sorted(entries.items()):
            info = zipfile.ZipInfo(name, ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.flag_bits = 0
            archive.writestr(info, payload, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    return output.getvalue()


def _wheel_directory(path: str) -> Path:
    directory = Path(path)
    if not directory.is_absolute():
        directory = Path(os.getcwd()) / directory
    return directory


def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    """Build one deterministic pure-Python wheel; never overwrite an existing artifact."""
    del config_settings, metadata_directory
    directory = _wheel_directory(os.fspath(wheel_directory))
    directory_fd = _open_directory_chain(directory)
    fd = -1
    try:
        payload = _wheel()
        fd = os.open(WHEEL_FILENAME,
                     os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW |
                     getattr(os, "O_CLOEXEC", 0), 0o644, dir_fd=directory_fd)
        with os.fdopen(fd, "wb", closefd=False) as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    except FileExistsError as exc:
        raise FileExistsError(f"refusing to overwrite existing wheel: {WHEEL_FILENAME}") from exc
    except OSError:
        if fd >= 0:
            os.close(fd)
            fd = -1
        # Preserve the create-new name on failure: unlinking by name could delete a concurrent replacement.
        raise
    finally:
        if fd >= 0:
            os.close(fd)
        os.close(directory_fd)
    return WHEEL_FILENAME


def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):
    """Write deterministic dist-info metadata for frontends that request this hook."""
    del config_settings
    directory = _wheel_directory(os.fspath(metadata_directory))
    metadata = _metadata(_read_source("README.md"))
    wheel = (
        b"Wheel-Version: 1.0\nGenerator: openatca-stdlib-wheel-backend\n"
        b"Root-Is-Purelib: true\nTag: py3-none-any\n"
    )
    license_bytes = _read_source("LICENSE")
    directory_fd = _open_directory_chain(directory)
    dist_fd = -1
    licenses_fd = -1
    try:
        os.mkdir(DIST_INFO, mode=0o755, dir_fd=directory_fd)
        dist_fd = os.open(DIST_INFO, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                          dir_fd=directory_fd)
        os.mkdir("licenses", mode=0o755, dir_fd=dist_fd)
        licenses_fd = os.open("licenses", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                              dir_fd=dist_fd)
        _write_new_at(dist_fd, "METADATA", metadata)
        _write_new_at(dist_fd, "WHEEL", wheel)
        _write_new_at(licenses_fd, "LICENSE", license_bytes)
    finally:
        if licenses_fd >= 0:
            os.close(licenses_fd)
        if dist_fd >= 0:
            os.close(dist_fd)
        os.close(directory_fd)
    return DIST_INFO


def _write_new_at(directory_fd: int, name: str, payload: bytes) -> None:
    fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW |
                 getattr(os, "O_CLOEXEC", 0), 0o644, dir_fd=directory_fd)
    try:
        with os.fdopen(fd, "wb", closefd=False) as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        # A partial create-new artifact is retained on failure; name-based cleanup can race replacement.
        os.close(fd)
