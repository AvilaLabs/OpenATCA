#!/usr/bin/env python3
"""Verify the exact bounded wheel artifact before the trusted-publisher step."""

from __future__ import annotations

import base64
import csv
import hashlib
import io
from pathlib import Path
import stat
import sys
import zipfile
from email.parser import BytesParser
from email.policy import default as email_policy


WHEEL_NAME = "openatca-0.1.0-py3-none-any.whl"
MAX_MEMBER = 4 * 1024 * 1024
MAX_TOTAL = 10 * 1024 * 1024
MAX_WHEEL_BYTES = 16 * 1024 * 1024
MEMBERS = {
    "openatca/__init__.py",
    "openatca/py.typed",
    "openatca-0.1.0.dist-info/licenses/LICENSE",
    "openatca-0.1.0.dist-info/METADATA",
    "openatca-0.1.0.dist-info/WHEEL",
    "openatca-0.1.0.dist-info/RECORD",
}
RECORD = "openatca-0.1.0.dist-info/RECORD"


def verify(dist: Path) -> None:
    if dist.is_symlink() or not dist.is_dir():
        raise ValueError("distribution path must be a real directory")
    entries = list(dist.iterdir())
    if len(entries) != 1 or entries[0].name != WHEEL_NAME:
        raise ValueError("distribution must contain exactly the expected wheel")
    wheel = entries[0]
    try:
        wheel_stat = wheel.lstat()
    except OSError as exc:
        raise ValueError("wheel is unavailable") from exc
    if not stat.S_ISREG(wheel_stat.st_mode):
        raise ValueError("wheel must be a regular file")
    if wheel_stat.st_size > MAX_WHEEL_BYTES:
        raise ValueError("compressed wheel exceeds the byte limit")
    with zipfile.ZipFile(wheel) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if len(names) != len(set(names)) or set(names) != MEMBERS:
            raise ValueError("wheel member inventory differs from the approved package")
        payloads: dict[str, bytes] = {}
        total = 0
        for info in infos:
            mode = info.external_attr >> 16
            if info.is_dir() or stat.S_ISLNK(mode) or info.file_size > MAX_MEMBER:
                raise ValueError("wheel contains a directory, link, or oversized member")
            total += info.file_size
            if total > MAX_TOTAL:
                raise ValueError("wheel uncompressed size exceeds the package limit")
            payload = archive.read(info)
            if len(payload) != info.file_size:
                raise ValueError("wheel member size does not match its directory entry")
            payloads[info.filename] = payload

        if archive.testzip() is not None:
            raise ValueError("wheel CRC check failed")
        try:
            metadata = BytesParser(policy=email_policy).parsebytes(
                payloads["openatca-0.1.0.dist-info/METADATA"], headersonly=True
            )
            wheel_metadata = BytesParser(policy=email_policy).parsebytes(
                payloads["openatca-0.1.0.dist-info/WHEEL"], headersonly=True
            )
            rows = list(csv.reader(io.StringIO(payloads[RECORD].decode("utf-8"), newline=""), strict=True))
        except (UnicodeDecodeError, csv.Error, ValueError) as exc:
            raise ValueError("wheel metadata or RECORD is malformed") from exc

        def require_header(message, name: str, expected: str) -> None:
            values = message.get_all(name, [])
            if message.defects or values != [expected]:
                raise ValueError(f"wheel header {name} is missing, duplicated, or incorrect")

        require_header(metadata, "Metadata-Version", "2.4")
        require_header(metadata, "Name", "openatca")
        require_header(metadata, "Version", "0.1.0")
        require_header(wheel_metadata, "Wheel-Version", "1.0")
        require_header(wheel_metadata, "Generator", "openatca-stdlib-wheel-backend")
        require_header(wheel_metadata, "Root-Is-Purelib", "true")
        require_header(wheel_metadata, "Tag", "py3-none-any")

        expected_rows: dict[str, tuple[str, str]] = {}
        for name, payload in payloads.items():
            if name == RECORD:
                continue
            digest = base64.urlsafe_b64encode(hashlib.sha256(payload).digest()).rstrip(b"=").decode("ascii")
            expected_rows[name] = (f"sha256={digest}", str(len(payload)))
        actual_rows: dict[str, tuple[str, str]] = {}
        for row in rows:
            if len(row) != 3 or row[0] in actual_rows:
                raise ValueError("wheel RECORD has malformed or duplicate rows")
            actual_rows[row[0]] = (row[1], row[2])
        if actual_rows.pop(RECORD, None) != ("", "") or actual_rows != expected_rows:
            raise ValueError("wheel RECORD hashes or sizes do not match the members")


def main() -> int:
    try:
        if len(sys.argv) != 2:
            raise ValueError("usage: verify_python_wheel.py DIST_DIR")
        verify(Path(sys.argv[1]))
        print("wheel inventory and RECORD verified")
        return 0
    except (OSError, ValueError, zipfile.BadZipFile) as exc:
        print(f"wheel verification refused: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
