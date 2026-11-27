#!/usr/bin/env python3
"""Prepare an atomic, locally verified transfer bundle for the approved observer package.

This script only reads the package source and the two explicit binary paths. It does not install,
enable, start, contact a host, inspect subscription files, or copy any existing configuration.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tempfile
from pathlib import Path


MAX_PACKAGE_FILE_BYTES = 64 * 1024 * 1024
MAX_FEED_BYTES = 256 * 1024 * 1024
MAX_BRIDGE_JAR_BYTES = 128 * 1024 * 1024
COPY_CHUNK_BYTES = 1024 * 1024
SHA256_RE = re.compile(r"[0-9a-fA-F]{64}\Z")
PACKAGE_FILES = (
    ("README.md", "package/README.md", False),
    ("install-user.sh", "package/install-user.sh", True),
    ("collect-one.sh", "package/collect-one.sh", True),
    ("run-observer.sh", "package/run-observer.sh", True),
    ("oaf_observer.py", "package/oaf_observer.py", True),
    ("check-disk.py", "package/check-disk.py", True),
    ("storage-budget.env.example", "config/storage-budget.env.example", False),
    (
        "systemd/user/openatca-collector@.service",
        "package/systemd/user/openatca-collector@.service",
        False,
    ),
    (
        "systemd/user/openatca-observer.service",
        "package/systemd/user/openatca-observer.service",
        False,
    ),
)


class Refusal(Exception):
    pass


class PublishedDurabilityUncertain(Exception):
    def __init__(self, path: Path, cause: OSError):
        super().__init__(f"parent-directory fsync failed: {cause}")
        self.path = path


def digest_arg(value: str, label: str) -> str:
    if not SHA256_RE.fullmatch(value):
        raise Refusal(f"{label} must be a 64-character SHA-256 hex digest")
    return value.lower()


def checked_regular_file(path: Path, maximum: int, label: str) -> tuple[int, os.stat_result]:
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_NONBLOCK"):
        flags |= os.O_NONBLOCK
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise Refusal(f"cannot open {label}: {exc}") from exc
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise Refusal(f"{label} must be a regular file")
        if before.st_size < 0 or before.st_size > maximum:
            raise Refusal(f"{label} exceeds its {maximum}-byte input bound")
        return fd, before
    except Exception:
        os.close(fd)
        raise


def copy_hashed(source: Path, destination: Path, bundle_root: Path, maximum: int, label: str,
                expected_sha256: str | None = None, executable: bool = False) -> dict[str, object]:
    fd, before = checked_regular_file(source, maximum, label)
    digest = hashlib.sha256()
    copied = 0
    try:
        with os.fdopen(fd, "rb", closefd=True) as reader:
            fd = -1
            with destination.open("xb") as writer:
                while True:
                    chunk = reader.read(COPY_CHUNK_BYTES)
                    if not chunk:
                        break
                    copied += len(chunk)
                    if copied > maximum or copied > before.st_size:
                        raise Refusal(f"{label} changed or exceeded its bounded read")
                    digest.update(chunk)
                    writer.write(chunk)
                writer.flush()
                os.fsync(writer.fileno())
            after = os.fstat(reader.fileno())
            if (copied != before.st_size or after.st_size != before.st_size
                    or after.st_mtime_ns != before.st_mtime_ns
                    or after.st_ctime_ns != before.st_ctime_ns):
                raise Refusal(f"{label} changed while it was copied")
        actual = digest.hexdigest()
        if expected_sha256 is not None and actual != expected_sha256:
            raise Refusal(f"{label} SHA-256 does not match the supplied pin")
        os.chmod(destination, 0o700 if executable else 0o600)
        return {"path": destination.relative_to(bundle_root).as_posix(),
                "bytes": copied, "sha256": actual}
    except Exception:
        if fd >= 0:
            os.close(fd)
        try:
            destination.unlink()
        except FileNotFoundError:
            pass
        raise


def write_text(path: Path, text: str, mode: int = 0o600) -> None:
    data = text.encode("utf-8")
    if len(data) > MAX_PACKAGE_FILE_BYTES:
        raise Refusal(f"generated file exceeds its size bound: {path.name}")
    with path.open("xb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    os.chmod(path, mode)


def fsync_directory(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def transfer_notes() -> str:
    return """# Observer transfer bundle

This is a local preparation artifact. Creating it does not transfer, install, enable, or start
anything. No destination has been selected. The included setup script is provided for a later,
separately reviewed local install; it does not enable or start services.

## Inventory and input pins

`SHA256SUMS.json` inventories every copied package file, the explicitly supplied `oa-feed` binary,
and the explicitly supplied SCDS bridge JAR. Its `oa_feed` and `bridge_jar` hashes were checked
against the mandatory pins supplied to the bundle-preparation command. Verify the inventory again
after any later transfer.

## Configuration and storage

Only `config/storage-budget.env.example` is included. It contains zero placeholders and is not a
usable runtime budget. Do not bundle or copy existing source subscription files. For each of
SFDPS, STDDS, and TBFM, an operator must measure local uncompressed journal growth in integer
bytes/hour and enter positive values in a separately created `storage-budget.env`. Preflight
requires the per-feed 2 GiB cap to cover 200% of the measured rate for 720 hours, and requires
free disk for all three projected budgets plus a 512 MiB reserve. Missing/zero measurements fail
before a collector opens a subscription.

## Fixed acquisition bounds

The included observer unit uses `[2026-11-30T05:00:00Z, 2026-12-01T06:00:00Z)`.
The P010 warm-up interval `[2026-11-30T05:00:00Z, 2026-11-30T06:00:00Z)` is excluded from
scoring. The scored interval is `[2026-11-30T06:00:00Z, 2026-12-01T00:00:00Z)`; the remaining
acquired interval is explicitly post-score. Do not widen these bounds in this bundle.

## Deployment procedure for the project operator (TODO: destination details required)

1. Obtain the approved destination account, host, and transfer directory from the user. Do not
   infer them from machine state or attempt a connection before those details are supplied.
2. Verify `SHA256SUMS.json` locally and after transfer. Confirm the binary and JAR hashes match
   the reviewed pins; do not substitute a newly built binary or JAR.
3. Separately place already-approved subscription files at the target user's documented source
   configuration paths. Keep their contents out of this bundle and all manifests.
4. Measure each feed's local uncompressed journal growth, prepare the real storage budget from the
   template, and confirm the package preflight accepts the actual caps and free-space reserve.
5. Review the fixed UTC acquisition and scoring windows, target service hardening, user lingering,
   and service logs. A later installation or service start requires a separate explicit request.
6. Do not represent local observations as facility operation or complete capture until the
   package's close/check procedure verifies all three feeds through the declared end.
"""


def prepare(args: argparse.Namespace) -> Path:
    out = Path(args.output).expanduser()
    parent = out.parent.resolve(strict=True)
    final = parent / out.name
    if final.exists() or final.is_symlink():
        raise Refusal("output path already exists; refusing to overwrite it")
    if final.name in ("", ".", ".."):
        raise Refusal("output must name a new directory")
    root = Path(__file__).resolve(strict=True).parent
    feed_pin = digest_arg(args.oa_feed_sha256, "oa-feed pin")
    jar_pin = digest_arg(args.bridge_jar_sha256, "bridge JAR pin")
    stage = Path(tempfile.mkdtemp(prefix=f".{final.name}.stage-", dir=parent))
    os.chmod(stage, 0o700)
    try:
        inventory: list[dict[str, object]] = []
        for source_rel, dest_rel, executable in PACKAGE_FILES:
            source = root / source_rel
            destination = stage / dest_rel
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            inventory.append(copy_hashed(source, destination, stage, MAX_PACKAGE_FILE_BYTES,
                                         f"package file {source_rel}", executable=executable))
        feed_source = Path(args.oa_feed).expanduser()
        feed_dest = stage / "bin/oa-feed"
        feed_dest.parent.mkdir(mode=0o700)
        inventory.append(copy_hashed(feed_source, feed_dest, stage, MAX_FEED_BYTES, "oa-feed binary",
                                     feed_pin, executable=True))
        jar_source = Path(args.bridge_jar).expanduser()
        jar_dest = stage / "lib/scds-bridge.jar"
        jar_dest.parent.mkdir(mode=0o700)
        inventory.append(copy_hashed(jar_source, jar_dest, stage, MAX_BRIDGE_JAR_BYTES,
                                     "SCDS bridge JAR", jar_pin))
        write_text(stage / "DEPLOYMENT-PROCEDURE-TODO.md", transfer_notes())
        inventory.append({"path": "DEPLOYMENT-PROCEDURE-TODO.md", "generated": True})
        inventory.sort(key=lambda item: str(item["path"]))
        manifest = {
            "schema": "openatca-observer-transfer-bundle-v1",
            "files": inventory,
            "external_inputs": {
                "oa_feed_sha256_pin_verified": feed_pin,
                "bridge_jar_sha256_pin_verified": jar_pin,
                "subscription_configuration": "not included; configure separately",
                "storage_budget": "template only; requires measured positive per-feed values",
            },
            "capture_acquisition_utc": ["2026-11-30T05:00:00Z", "2026-12-01T06:00:00Z"],
            "capture_acquisition_duration_hours": 25,
            "warmup_excluded_from_scoring_utc": [
                "2026-11-30T05:00:00Z", "2026-11-30T06:00:00Z"
            ],
            "scored_interval_utc": ["2026-11-30T06:00:00Z", "2026-12-01T00:00:00Z"],
            "scored_interval_duration_hours": 18,
            "storage_planning_horizon_hours": 720,
            "prepared_only": True,
            "transfer_or_service_action_performed": False,
        }
        write_text(stage / "SHA256SUMS.json",
                   json.dumps(manifest, sort_keys=True, indent=2) + "\n")
        fsync_directory(stage)
        if final.exists() or final.is_symlink():
            raise Refusal("output path appeared during preparation; refusing to publish")
        os.rename(stage, final)
        try:
            fsync_directory(parent)
        except OSError as exc:
            raise PublishedDurabilityUncertain(final, exc) from exc
        return final
    except PublishedDurabilityUncertain:
        raise
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oa-feed", required=True, help="explicit path to the approved binary")
    parser.add_argument("--oa-feed-sha256", required=True, help="reviewed binary SHA-256 pin")
    parser.add_argument("--bridge-jar", required=True, help="explicit path to the approved JAR")
    parser.add_argument("--bridge-jar-sha256", required=True, help="reviewed JAR SHA-256 pin")
    parser.add_argument("--output", required=True, help="new local bundle directory")
    args = parser.parse_args(argv)
    try:
        result = prepare(args)
    except PublishedDurabilityUncertain as exc:
        print(
            f"bundle was published at {exc.path}, but parent-directory fsync failed; "
            "durability is uncertain; preserve it for inspection and do not assume it is absent",
            file=sys.stderr,
        )
        return 3
    except (OSError, Refusal) as exc:
        print(f"transfer bundle refused: {exc}", file=sys.stderr)
        return 2
    print(f"prepared local transfer bundle: {result}")
    print("No transfer, install, enable, or service start was performed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
