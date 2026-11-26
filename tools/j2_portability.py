#!/usr/bin/env python3
"""Export and replay a private, hash-bound regional-session setup for a fresh clone."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import struct
import subprocess
import sys

MAX_NORMALIZED_BYTES = 16 * 1024 * 1024
MAX_PROVENANCE_BYTES = 32 * 1024 * 1024
MAX_QUARANTINE_BYTES = 64 * 1024 * 1024
MAX_COUNTS_BYTES = 64 * 1024
MAX_IMPORT_MANIFEST_BYTES = 1024 * 1024
MAX_ADAPTATION_BYTES = 512 * 1024 * 1024
MAX_ADAPTATION_MANIFEST_BYTES = 512
FRAME_WIDTH = 800
FRAME_HEIGHT = 600
MAX_FRAME_PPM_BYTES = len(b"P6\n800 600\n255\n") + FRAME_WIDTH * FRAME_HEIGHT * 3
MAX_SETTINGS_BYTES = 64 * 1024
MAX_RUN_MANIFEST_BYTES = 64 * 1024 * 1024
MAX_TOTAL_BUNDLE_BYTES = 2 * 1024 * 1024 * 1024
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_BUILD_RECEIPT_BYTES = 64 * 1024
MAX_COLD_EXECUTABLE_BYTES = 512 * 1024 * 1024
CHUNK_BYTES = 64 * 1024
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
HEX40 = re.compile(r"[0-9a-f]{40}\Z")
RUST_TOOLCHAIN = "1.89.0"
RECORDED_PROFILE_ID = "RECORDED_PEAK_RENDERED_V1"
RECORDED_PROFILE_SETTINGS = {
    "profile": RECORDED_PROFILE_ID,
    "tracks": "4096",
    "flights": "4096",
    "frames": "512",
    "surveillance_max_frames": "20000000",
    "max_total_events": "10000000",
    "max_drain_rows": "262144",
    "max_baro_evidence": "1000000",
    "max_open_alerts": "1048576",
    "max_candidate_pairs": "8388608",
    "max_ca_work_per_frame": "8388608",
    "max_flight_log_entries": "1024",
    "max_presentation_updates": "8192",
    "render_scope": "true",
}

SCRIPT = Path(__file__).resolve(strict=True)
GUARD_SPEC = importlib.util.spec_from_file_location("j2_sealed_guard", SCRIPT.with_name("sealed_guard.py"))
if GUARD_SPEC is None or GUARD_SPEC.loader is None:
    raise RuntimeError("cannot load the existing sealed path guard")
sealed_guard = importlib.util.module_from_spec(GUARD_SPEC)
GUARD_SPEC.loader.exec_module(sealed_guard)
GUARD = sealed_guard.production()


class DrillError(Exception):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require_private_path(path: Path, *, new: bool = False) -> Path:
    try:
        GUARD.check(path)
        if new and path.is_symlink():
            raise DrillError("new output path must not be a symlink")
        resolved = path.resolve(strict=not new)
    except (OSError, sealed_guard.SealedError) as exc:
        raise DrillError(f"guarded path refusal: {exc}") from exc
    if "openatca-private" not in resolved.parts:
        raise DrillError("setup inputs and outputs must stay under an openatca-private directory")
    if new and (resolved.exists() or resolved.is_symlink()):
        raise DrillError("output must be a new path")
    return resolved


def guarded_existing_path(path: Path, label: str) -> Path:
    try:
        GUARD.check(path)
        resolved = path.resolve(strict=True)
    except (OSError, ValueError, sealed_guard.SealedError) as exc:
        raise DrillError(f"guarded {label} refusal: {exc}") from exc
    return resolved


def private_directory_candidate(path: Path) -> Path:
    try:
        GUARD.check(path)
        if path.is_symlink():
            raise DrillError("private work directory must not be a symlink")
        resolved = path.resolve(strict=False)
    except (OSError, ValueError, sealed_guard.SealedError) as exc:
        raise DrillError(f"guarded private directory refusal: {exc}") from exc
    if "openatca-private" not in resolved.parts:
        raise DrillError("cold-build directories must stay under an openatca-private directory")
    return resolved


def require_disjoint_paths(named_paths: tuple[tuple[str, Path], ...]) -> None:
    for index, (left_name, left) in enumerate(named_paths):
        for right_name, right in named_paths[index + 1 :]:
            if left == right or left in right.parents or right in left.parents:
                raise DrillError(f"{left_name} and {right_name} paths must be disjoint")


def make_empty_private_dir(path: Path) -> Path:
    resolved = private_directory_candidate(path)
    if resolved.exists():
        mode = stat.S_IMODE(resolved.stat().st_mode)
        if not resolved.is_dir() or mode & 0o077 or any(resolved.iterdir()):
            raise DrillError("cold-build directory must be an empty real directory")
    else:
        resolved.mkdir(mode=0o700, parents=True)
    return resolved


def read_bounded(path: Path, limit: int) -> bytes:
    try:
        with GUARD.open_regular_read(path, "rb") as stream:
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
                raise DrillError(f"input exceeds its fixed byte limit: {path.name}")
            data = stream.read(limit + 1)
            after = os.fstat(stream.fileno())
    except (OSError, sealed_guard.SealedError) as exc:
        raise DrillError(f"guarded input refusal: {exc}") from exc
    if len(data) > limit or len(data) != before.st_size or (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino):
        raise DrillError(f"input changed or exceeded its fixed byte limit: {path.name}")
    return data


def copy_bounded(source: Path, destination: Path, limit: int) -> dict[str, object]:
    try:
        with GUARD.open_regular_read(source, "rb") as src:
            before = os.fstat(src.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
                raise DrillError(f"input exceeds its fixed byte limit: {source.name}")
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
            fd = os.open(destination, flags, 0o600)
            digest = hashlib.sha256()
            total = 0
            try:
                with os.fdopen(fd, "wb", closefd=True) as dst:
                    while True:
                        block = src.read(CHUNK_BYTES)
                        if not block:
                            break
                        total += len(block)
                        if total > limit:
                            raise DrillError(f"input grew beyond its fixed byte limit: {source.name}")
                        digest.update(block)
                        dst.write(block)
                    dst.flush()
                    os.fsync(dst.fileno())
            except BaseException:
                try:
                    destination.unlink()
                except OSError:
                    pass
                raise
            after = os.fstat(src.fileno())
    except (OSError, sealed_guard.SealedError) as exc:
        raise DrillError(f"guarded copy refusal: {exc}") from exc
    if total != before.st_size or (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino):
        destination.unlink(missing_ok=True)
        raise DrillError(f"input changed while copying: {source.name}")
    return {"bytes": total, "sha256": digest.hexdigest()}


def parse_tsv_fields(data: bytes, header: str, max_lines: int) -> list[list[str]]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise DrillError("an input metadata file is not UTF-8") from exc
    if not text.endswith("\n"):
        raise DrillError("an input metadata file is not newline terminated")
    lines = text.splitlines()
    if not lines or lines[0] != header or len(lines) > max_lines:
        raise DrillError("an input metadata file has the wrong header or exceeds its row limit")
    return [line.split("\t") for line in lines[1:] if line]


def unique_fields(lines: list[list[str]], keys: set[str]) -> dict[str, str]:
    found: dict[str, str] = {}
    for row in lines:
        if not row or row[0] not in keys:
            continue
        if len(row) != 2 or row[0] in found:
            raise DrillError(f"invalid or repeated metadata field: {row[0] if row else 'empty'}")
        found[row[0]] = row[1]
    missing = keys - found.keys()
    if missing:
        raise DrillError("required metadata field is missing")
    return found


def verify_complete(directory: Path, manifest_name: str, complete_name: str, limit: int) -> tuple[bytes, str]:
    manifest = read_bounded(directory / manifest_name, limit)
    complete = read_bounded(directory / complete_name, 128)
    expected = (sha256(manifest) + "\n").encode("ascii")
    if complete != expected:
        raise DrillError(f"{complete_name} does not bind {manifest_name}")
    return manifest, sha256(manifest)


def validate_recording_import(directory: Path) -> tuple[dict[str, bytes], str]:
    manifest, manifest_hash = verify_complete(
        directory, "manifest.tsv", "COMPLETE", MAX_IMPORT_MANIFEST_BYTES
    )
    rows = parse_tsv_fields(manifest, "OA_RECORDING_IMPORT_MANIFEST\t1", 128)
    fields = unique_fields(rows, {
        "centre", "normalized_bytes", "normalized_sha256",
        "provenance_bytes", "provenance_sha256",
        "quarantine_bytes", "quarantine_sha256",
        "report_bytes", "report_sha256",
    })
    if fields["centre"] != "ZAU1":
        raise DrillError("recording import centre must be ZAU1")
    index_rows = [row for row in rows if row and row[0] == "input_stream"]
    if not index_rows or any(len(row) != 7 for row in index_rows):
        raise DrillError("recording index must name at least one complete source stream")
    stream_ids: set[str] = set()
    for row in index_rows:
        if (not row[1].isdecimal() or row[1] in stream_ids
                or row[2] not in {"ADSB", "SFDPS"} or not HEX64.fullmatch(row[3])
                or not row[4].isdecimal() or not row[5].isdecimal() or row[6] not in {"0", "1"}):
            raise DrillError("recording index contains an invalid source stream entry")
        stream_ids.add(row[1])
    normalized = read_bounded(directory / "normalized.tsv", MAX_NORMALIZED_BYTES)
    if len(normalized) != int(fields["normalized_bytes"]) or sha256(normalized) != fields["normalized_sha256"]:
        raise DrillError("normalized input does not match its recording-index manifest")
    sidecars: dict[str, bytes] = {}
    for filename, label, limit in (
        ("provenance.tsv", "provenance", MAX_PROVENANCE_BYTES),
        ("quarantine.tsv", "quarantine", MAX_QUARANTINE_BYTES),
        ("counts.tsv", "report", MAX_COUNTS_BYTES),
    ):
        byte_count = fields[f"{label}_bytes"]
        digest = fields[f"{label}_sha256"]
        if not byte_count.isascii() or not byte_count.isdecimal() or not HEX64.fullmatch(digest):
            raise DrillError(f"{label} metadata has an invalid byte count or digest")
        payload = read_bounded(directory / filename, limit)
        if len(payload) != int(byte_count) or sha256(payload) != digest:
            raise DrillError(f"{label} sidecar does not match its recording-index manifest")
        sidecars[filename] = payload
    try:
        normalized_text = normalized.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise DrillError("normalized replay input is not UTF-8") from exc
    normalized_lines = normalized_text.splitlines()
    if not normalized_lines or normalized_lines[0] != "OA-RS\t1" or len(normalized_lines) < 2:
        raise DrillError("normalized replay input has no event rows")
    return {
        "recording/normalized.tsv": normalized,
        "recording/provenance.tsv": sidecars["provenance.tsv"],
        "recording/quarantine.tsv": sidecars["quarantine.tsv"],
        "recording/counts.tsv": sidecars["counts.tsv"],
        "recording/index.tsv": manifest,
        "recording/COMPLETE": (manifest_hash + "\n").encode("ascii"),
    }, manifest_hash


def parse_regional_manifest(manifest: bytes) -> tuple[dict[str, str], list[list[str]]]:
    rows = parse_tsv_fields(manifest, "OA_REGIONAL_MANIFEST\t1", 100_000)
    fields: dict[str, str] = {}
    frames: list[list[str]] = []
    for row in rows:
        if row[0] == "frame":
            if len(row) != 6:
                raise DrillError("regional replay frame manifest entry is malformed")
            frames.append(row)
        elif len(row) == 2 and row[0] in {"input_sha256", "snapshot_sha256", "executable_sha256", "settings_sha256"}:
            if row[0] in fields:
                raise DrillError("regional replay manifest contains a repeated digest")
            fields[row[0]] = row[1]
    if len(frames) == 0 or set(fields) != {"input_sha256", "snapshot_sha256", "executable_sha256", "settings_sha256"}:
        raise DrillError("regional replay manifest is incomplete or has zero frames")
    if any(not HEX64.fullmatch(value) for value in fields.values()):
        raise DrillError("regional replay manifest has an invalid digest")
    for frame in frames:
        if not HEX64.fullmatch(frame[4]) or not HEX64.fullmatch(frame[5]):
            raise DrillError("regional replay frame manifest has an invalid digest")
    return fields, frames


def regional_run_profile(settings: bytes) -> tuple[str, str | None]:
    """Return the validated stored CLI profile and optional CLI selector.

    Historic settings without a profile row replay with unchanged defaults. The recorded profile
    is accepted only with its exact versioned capacity tuple, so a digest-bound settings file
    cannot ask a fresh clone to run a different configuration under the same profile label.
    """
    try:
        text = settings.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise DrillError("regional settings are not UTF-8") from exc
    lines = text.splitlines()
    if not lines or lines[0] != "OA_REGIONAL_SETTINGS\t1":
        raise DrillError("regional settings have an unsupported header")
    fields: dict[str, str] = {}
    for line in lines[1:]:
        parts = line.split("\t", 1)
        if len(parts) != 2 or not parts[0] or parts[0] in fields:
            raise DrillError("regional settings contain a malformed or duplicate field")
        fields[parts[0]] = parts[1]
    profile = fields.get("profile")
    if profile is None:
        return "DEFAULT_V1", None
    if profile != RECORDED_PROFILE_ID:
        raise DrillError("regional settings name an unsupported run profile")
    if any(fields.get(name) != value for name, value in RECORDED_PROFILE_SETTINGS.items()):
        raise DrillError("recorded regional profile capacities do not match the selected profile")
    return RECORDED_PROFILE_ID, "recorded-peak-rendered"


def validate_adaptation_package(manifest: bytes, snapshot: bytes) -> None:
    # OAG2ADPT v1 layout is fixed by tools/oa-adapt/src/manifest.rs.
    if len(manifest) < 58 or manifest[:8] != b"OAG2ADPT":
        raise DrillError("adaptation package manifest is truncated or unsupported")
    version, reader_min, reader_max = struct.unpack_from("<HHH", manifest, 8)
    if version != 1:
        raise DrillError(f"unsupported OAG2 adaptation manifest version {version}; only v1 is implemented")
    release_len, facility_len, change_len, component_count = manifest[14:18]
    snapshot_len = struct.unpack_from("<Q", manifest, 18)[0]
    snapshot_hash = manifest[26:58]
    end = 58 + release_len + facility_len + change_len
    if (reader_min > reader_max or reader_min > 1 or reader_max < 1 or component_count != 1
            or release_len == 0 or release_len > 64 or facility_len == 0 or facility_len > 64
            or change_len == 0 or change_len > 128 or end != len(manifest)
            or snapshot_len > MAX_ADAPTATION_BYTES or snapshot_len != len(snapshot)
            or snapshot_hash.hex() != sha256(snapshot)):
        raise DrillError("adaptation package manifest does not bind the ZAU snapshot")
    release = manifest[58:58 + release_len]
    facility = manifest[58 + release_len:58 + release_len + facility_len]
    change_ref = manifest[58 + release_len + facility_len:end]
    for label, token in (("release", release), ("facility", facility), ("change reference", change_ref)):
        try:
            text = token.decode("ascii")
        except UnicodeDecodeError as exc:
            raise DrillError(f"adaptation manifest {label} is invalid") from exc
        if (not text or any(not (ch.isascii() and (ch.isalnum() or ch in "-_./")) for ch in text)
                or any(part in {"", ".", ".."} for part in text.split("/"))):
            raise DrillError(f"adaptation manifest {label} is invalid")
    if facility != b"ZAU":
        raise DrillError("adaptation package is not for ZAU")
    if len(snapshot) < 31 or snapshot[:11] != b"OATCA-ADAPT":
        raise DrillError("adaptation snapshot has an unsupported header")
    version = struct.unpack_from("<I", snapshot, 11)[0]
    if version not in (3, 4):
        raise DrillError("adaptation snapshot version is unsupported")
    artcc_len = struct.unpack_from("<I", snapshot, 27)[0]
    artcc_end = 31 + artcc_len
    if (artcc_len == 0 or artcc_len > 64 or artcc_end > len(snapshot)
            or snapshot[31:artcc_end] != b"ZAU"):
        raise DrillError("adaptation snapshot is not for ZAU")


def verify_frame_files(directory: Path, frames: list[list[str]]) -> None:
    if len(frames) > 100_000:
        raise DrillError("regional replay frame count exceeds its fixed validation limit")
    for row in frames:
        sequence, receipt_ns, evidence_name, evidence_sha, image_sha = row[1:6]
        if (not sequence.isdecimal() or not receipt_ns.isdecimal()
                or evidence_name != f"frame-{int(sequence):04}.txt"):
            raise DrillError("regional replay manifest has an invalid sequence, receipt time, or frame filename")
        evidence = read_bounded(directory / evidence_name, 64 * 1024)
        image = read_bounded(directory / f"frame-{int(sequence):04}.ppm", MAX_FRAME_PPM_BYTES)
        if len(image) != MAX_FRAME_PPM_BYTES or not image.startswith(b"P6\n800 600\n255\n"):
            raise DrillError("regional replay PPM dimensions or byte length are unsupported")
        if sha256(evidence) != evidence_sha or sha256(image) != image_sha:
            raise DrillError("regional replay frame file digest mismatch")


def validate_baseline(directory: Path, input_hash: str, snapshot_hash: str) -> tuple[dict[str, bytes], str]:
    manifest, manifest_hash = verify_complete(directory, "manifest.tsv", "COMPLETE", MAX_RUN_MANIFEST_BYTES)
    fields, frames = parse_regional_manifest(manifest)
    settings = read_bounded(directory / "settings.tsv", MAX_SETTINGS_BYTES)
    if not settings.startswith(b"OA_REGIONAL_SETTINGS\t1\n"):
        raise DrillError("baseline replay settings have an unsupported header")
    if fields["input_sha256"] != input_hash or fields["snapshot_sha256"] != snapshot_hash:
        raise DrillError("baseline replay does not use the bundled normalized input and adaptation")
    if fields["settings_sha256"] != sha256(settings):
        raise DrillError("baseline replay settings digest mismatch")
    if b"centre\tZAU1\n" not in settings:
        raise DrillError("baseline replay configuration is not for ZAU1")
    regional_run_profile(settings)
    verify_frame_files(directory, frames)
    return {
        "replay/settings.tsv": settings,
        "replay/manifest.tsv": manifest,
        "replay/COMPLETE": (manifest_hash + "\n").encode("ascii"),
    }, manifest_hash


def export_bundle(import_dir: Path, adaptation_dir: Path, baseline_dir: Path,
                  source_commit: str, output: Path) -> dict[str, object]:
    if not HEX40.fullmatch(source_commit):
        raise DrillError("source commit must be 40 lower-case hexadecimal digits")
    import_dir = require_private_path(import_dir)
    adaptation_dir = require_private_path(adaptation_dir)
    baseline_dir = require_private_path(baseline_dir)
    output = require_private_path(output, new=True)
    import_files, import_index_hash = validate_recording_import(import_dir)
    snapshot = read_bounded(adaptation_dir / "airspace.snapshot", MAX_ADAPTATION_BYTES)
    adaptation_manifest = read_bounded(adaptation_dir / "manifest.oag2", MAX_ADAPTATION_MANIFEST_BYTES)
    validate_adaptation_package(adaptation_manifest, snapshot)
    baseline_files, baseline_hash = validate_baseline(
        baseline_dir, sha256(import_files["recording/normalized.tsv"]), sha256(snapshot)
    )
    inputs = {
        **import_files,
        "adaptation/airspace.snapshot": snapshot,
        "adaptation/manifest.oag2": adaptation_manifest,
        **baseline_files,
    }
    total = sum(len(payload) for payload in inputs.values())
    if total > MAX_TOTAL_BUNDLE_BYTES:
        raise DrillError("bundle exceeds its fixed aggregate byte limit")
    os.mkdir(output, 0o700)
    inventory: dict[str, dict[str, object]] = {}
    try:
        for rel, payload in sorted(inputs.items()):
            if len(payload) > MAX_TOTAL_BUNDLE_BYTES:
                raise DrillError("bundle component exceeds the aggregate byte limit")
            path = output / rel
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0), 0o600)
            with os.fdopen(fd, "wb") as stream:
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
            inventory[rel] = {"bytes": len(payload), "sha256": sha256(payload)}
        manifest_obj = {
            "schema": "openatca-j2-regional-portability-v1",
            "source_commit": source_commit,
            "recording_index_sha256": import_index_hash,
            "baseline_manifest_sha256": baseline_hash,
            "files": inventory,
        }
        manifest = (json.dumps(manifest_obj, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
        if len(manifest) > MAX_MANIFEST_BYTES:
            raise DrillError("portability manifest exceeds its fixed byte limit")
        write_private(output / "PORTABILITY.json", manifest)
        write_private(output / "COMPLETE", (sha256(manifest) + "\n").encode("ascii"))
        return {"bundle_sha256": sha256(manifest), "files": len(inventory), "bytes": total}
    except BaseException:
        shutil.rmtree(output, ignore_errors=True)
        raise


def write_private(path: Path, data: bytes) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0), 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def load_bundle(bundle: Path) -> tuple[dict[str, object], bytes]:
    bundle = require_private_path(bundle)
    manifest = read_bounded(bundle / "PORTABILITY.json", MAX_MANIFEST_BYTES)
    complete = read_bounded(bundle / "COMPLETE", 128)
    if complete != (sha256(manifest) + "\n").encode("ascii"):
        raise DrillError("portability COMPLETE marker does not match its manifest")
    try:
        parsed = json.loads(manifest)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DrillError("portability manifest is invalid JSON") from exc
    if parsed.get("schema") != "openatca-j2-regional-portability-v1":
        raise DrillError("unsupported portability manifest schema")
    files = parsed.get("files")
    if not isinstance(files, dict) or not files:
        raise DrillError("portability manifest has no file inventory")
    return parsed, manifest


def verify_bundle_files(bundle: Path, parsed: dict[str, object]) -> dict[str, bytes]:
    raw_files = parsed.get("files")
    if not isinstance(raw_files, dict):
        raise DrillError("portability inventory is invalid")
    if len(raw_files) > 32:
        raise DrillError("portability inventory exceeds its fixed entry limit")
    result: dict[str, bytes] = {}
    total = 0
    for rel, record in sorted(raw_files.items()):
        if not isinstance(rel, str) or rel.startswith("/") or ".." in Path(rel).parts:
            raise DrillError("portability inventory contains an unsafe path")
        if not isinstance(record, dict) or not isinstance(record.get("bytes"), int) or not isinstance(record.get("sha256"), str):
            raise DrillError("portability file inventory entry is malformed")
        limit = MAX_TOTAL_BUNDLE_BYTES
        path = bundle / rel
        data = read_bounded(path, limit)
        if len(data) != record["bytes"] or sha256(data) != record["sha256"]:
            raise DrillError(f"bundle file digest mismatch: {rel}")
        total += len(data)
        if total > MAX_TOTAL_BUNDLE_BYTES:
            raise DrillError("bundle exceeds its fixed aggregate byte limit")
        result[rel] = data
    return result


def directory_digest(path: Path) -> tuple[int, str]:
    """Hash a bounded tree inventory without reading file contents."""
    entries: list[tuple[str, int, int, int]] = []
    count = 0
    for root, dirs, files in os.walk(path, followlinks=False):
        dirs.sort()
        files.sort()
        for name in dirs + files:
            item = Path(root) / name
            info = item.lstat()
            count += 1
            if count > 1_000_000 or stat.S_ISLNK(info.st_mode):
                raise DrillError("cold-build directory contains a symlink or exceeds its entry cap")
            rel = item.relative_to(path).as_posix()
            entries.append((rel, stat.S_IFMT(info.st_mode), info.st_size, info.st_mtime_ns))
    encoded = json.dumps(entries, separators=(",", ":"), ensure_ascii=True).encode("ascii")
    return count, sha256(encoded)


def run_git(repo: Path, *args: str, timeout: int = 30) -> str:
    try:
        result = subprocess.run(["git", "-C", str(repo), *args], check=True,
                                capture_output=True, text=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError) as exc:
        raise DrillError("git operation failed during the portability drill") from exc
    return result.stdout.strip()


def source_checkout(repo: Path) -> tuple[Path, str]:
    try:
        GUARD.check(repo)
        resolved = repo.resolve(strict=True)
    except (OSError, sealed_guard.SealedError) as exc:
        raise DrillError(f"guarded source checkout refusal: {exc}") from exc
    if not resolved.is_dir():
        raise DrillError("source checkout must be a directory")
    head = run_git(resolved, "rev-parse", "--verify", "HEAD")
    if not HEX40.fullmatch(head) or run_git(resolved, "status", "--porcelain=v1", "--untracked-files=all"):
        raise DrillError("source checkout must be clean at a full commit")
    return resolved, head


def require_executable_digest_matches(manifest_fields: dict[str, str], binary_sha256: str) -> None:
    if manifest_fields.get("executable_sha256") != binary_sha256:
        raise DrillError("regional run manifest executable digest differs from the executed binary")


def cold_cargo_executable(build_stdout: bytes, clone: Path, target_dir: Path) -> Path:
    """Pin the one fresh Cargo executable emitted by the fixed cold-build command."""
    expected = target_dir / "debug" / "oa-regional-session"
    manifest_dir = clone / "tools" / "oa-regional-session"
    expected_manifest = manifest_dir / "Cargo.toml"
    expected_src = manifest_dir / "src" / "main.rs"
    matches: list[Path] = []
    try:
        lines = build_stdout.decode("utf-8").splitlines()
        for line in lines:
            if not line:
                continue
            message = json.loads(line)
            if not isinstance(message, dict) or message.get("reason") != "compiler-artifact":
                continue
            target = message.get("target")
            if not isinstance(target, dict) or target.get("name") != "oa-regional-session":
                continue
            if message.get("manifest_path") != str(expected_manifest.resolve(strict=True)):
                raise DrillError("Cargo emitted the regional-session target from an unexpected manifest")
            if target.get("kind") != ["bin"] or target.get("src_path") != str(expected_src.resolve(strict=True)):
                raise DrillError("Cargo regional-session artifact has an unexpected target kind or source")
            if message.get("fresh") is not False:
                raise DrillError("Cargo regional-session executable was not freshly compiled")
            executable = message.get("executable")
            filenames = message.get("filenames")
            if not isinstance(executable, str) or Path(executable) != expected:
                raise DrillError("Cargo regional-session executable path differs from the cold target")
            if not isinstance(filenames, list) or not all(isinstance(name, str) for name in filenames):
                raise DrillError("Cargo regional-session artifact filenames are malformed")
            paths = tuple(Path(name) for name in filenames)
            if not paths or any(not path.is_absolute() or target_dir not in path.parents for path in paths):
                raise DrillError("Cargo artifact inventory escapes the isolated target directory")
            if expected not in paths:
                raise DrillError("Cargo artifact inventory omits the expected executable")
            matches.append(expected)
    except (UnicodeDecodeError, json.JSONDecodeError, OSError, ValueError) as exc:
        raise DrillError("Cargo artifact receipt is malformed") from exc
    if len(matches) != 1:
        raise DrillError("Cargo did not emit exactly one fresh regional-session executable")
    return matches[0]


def copy_fresh_cargo_executable(
    source: Path,
    destination: Path,
    expected_source: Path,
    target_dir: Path,
) -> dict[str, object]:
    """Copy only the pinned fresh Cargo executable, validating Cargo's local hardlink peer."""
    if (source != expected_source or source != target_dir / "debug" / "oa-regional-session"
            or destination != target_dir / "debug" / ".j2-replay" / "oa-regional-session"):
        raise DrillError("replay executable is not the exact cold Cargo artifact")
    created_copy = False
    peer_fd: int | None = None
    try:
        GUARD.check(source)
        if source.is_symlink() or not source.is_file():
            raise DrillError("cold Cargo executable must be a regular non-symlink file")
        before_path = source.lstat()
        if not stat.S_ISREG(before_path.st_mode) or before_path.st_size <= 0 or before_path.st_size > MAX_COLD_EXECUTABLE_BYTES:
            raise DrillError("cold Cargo executable is outside its fixed size bound")
        if not os.access(source, os.X_OK):
            raise DrillError("cold Cargo executable is not executable")
        fd = os.open(source, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NONBLOCK", 0))
        digest = hashlib.sha256()
        total = 0
        try:
            source_stat = os.fstat(fd)
            if not stat.S_ISREG(source_stat.st_mode) or (source_stat.st_dev, source_stat.st_ino) != (before_path.st_dev, before_path.st_ino):
                raise DrillError("cold Cargo executable changed before copy")
            if (source_stat.st_size != before_path.st_size or source_stat.st_mtime_ns != before_path.st_mtime_ns
                    or source_stat.st_nlink != before_path.st_nlink):
                raise DrillError("cold Cargo executable changed before copy")
            if source_stat.st_nlink == 2:
                deps_dir = target_dir / "debug" / "deps"
                GUARD.check(deps_dir)
                deps_stat = deps_dir.lstat()
                if not stat.S_ISDIR(deps_stat.st_mode) or stat.S_ISLNK(deps_stat.st_mode):
                    raise DrillError("Cargo dependency artifact directory is not a real directory")
                peers = []
                for entry in os.scandir(deps_dir):
                    if not re.fullmatch(r"oa_regional_session-[0-9a-f]{16}", entry.name):
                        continue
                    peer_stat = entry.stat(follow_symlinks=False)
                    if (peer_stat.st_dev, peer_stat.st_ino) == (source_stat.st_dev, source_stat.st_ino):
                        peers.append((Path(entry.path), peer_stat))
                if len(peers) != 1:
                    raise DrillError("Cargo hardlink peer was not unique inside the fresh dependency directory")
                peer, peer_path_stat = peers[0]
                if (not stat.S_ISREG(peer_path_stat.st_mode) or stat.S_ISLNK(peer_path_stat.st_mode)
                        or not (peer_path_stat.st_mode & 0o111) or peer_path_stat.st_nlink != 2):
                    raise DrillError("Cargo hardlink peer is not a single expected executable link")
                peer_fd = os.open(peer, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NONBLOCK", 0))
                peer_open_stat = os.fstat(peer_fd)
                if ((peer_open_stat.st_dev, peer_open_stat.st_ino) != (source_stat.st_dev, source_stat.st_ino)
                        or peer_open_stat.st_nlink != 2 or not stat.S_ISREG(peer_open_stat.st_mode)):
                    raise DrillError("opened Cargo hardlink peer changed or has the wrong identity")
            elif source_stat.st_nlink != 1:
                raise DrillError("cold Cargo executable has an unexpected hardlink count")
            fd_path = Path(os.readlink(f"/proc/self/fd/{fd}"))
            GUARD.check(fd_path)
            if fd_path != source.resolve(strict=True):
                raise DrillError("opened cold Cargo executable resolved to an unexpected path")
            replay_dir = destination.parent
            GUARD.check(replay_dir)
            replay_dir.mkdir(mode=0o700, parents=False, exist_ok=False)
            out_fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL
                             | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0), 0o700)
            created_copy = True
            try:
                with os.fdopen(out_fd, "wb", closefd=True) as output:
                    while True:
                        block = os.read(fd, CHUNK_BYTES)
                        if not block:
                            break
                        total += len(block)
                        if total > MAX_COLD_EXECUTABLE_BYTES:
                            raise DrillError("cold Cargo executable grew beyond its fixed size bound")
                        digest.update(block)
                        output.write(block)
                    output.flush()
                    os.fsync(output.fileno())
            except BaseException:
                destination.unlink(missing_ok=True)
                raise
            after_fd = os.fstat(fd)
            after_path = source.lstat()
            after_peer = os.fstat(peer_fd) if peer_fd is not None else None
        finally:
            if peer_fd is not None:
                os.close(peer_fd)
            os.close(fd)
        copied_stat = destination.lstat()
        if (total != source_stat.st_size or total != before_path.st_size
                or (source_stat.st_dev, source_stat.st_ino) != (after_fd.st_dev, after_fd.st_ino)
                or source_stat.st_nlink != after_fd.st_nlink
                or source_stat.st_size != after_fd.st_size
                or source_stat.st_mtime_ns != after_fd.st_mtime_ns
                or (after_path.st_dev, after_path.st_ino) != (source_stat.st_dev, source_stat.st_ino)
                or after_path.st_nlink != source_stat.st_nlink
                or after_path.st_size != source_stat.st_size
                or after_path.st_mtime_ns != source_stat.st_mtime_ns
                or (after_peer is not None and ((after_peer.st_dev, after_peer.st_ino) != (source_stat.st_dev, source_stat.st_ino)
                                                or after_peer.st_nlink != 2
                                                or after_peer.st_size != source_stat.st_size
                                                or after_peer.st_mtime_ns != source_stat.st_mtime_ns))
                or not stat.S_ISREG(copied_stat.st_mode) or copied_stat.st_nlink != 1
                or copied_stat.st_size != total or (copied_stat.st_mode & 0o111) == 0):
            destination.unlink(missing_ok=True)
            raise DrillError("cold Cargo executable changed or copied output failed verification")
        copied_sha256 = digest.hexdigest()
        try:
            with GUARD.open_regular_read(destination, "rb") as copied:
                copied_digest = hashlib.sha256()
                copied_total = 0
                while True:
                    block = copied.read(CHUNK_BYTES)
                    if not block:
                        break
                    copied_total += len(block)
                    if copied_total > MAX_COLD_EXECUTABLE_BYTES:
                        raise DrillError("single-link replay executable copy exceeded its fixed size bound")
                    copied_digest.update(block)
        except BaseException:
            destination.unlink(missing_ok=True)
            raise
        if copied_total != total or copied_digest.hexdigest() != copied_sha256:
            destination.unlink(missing_ok=True)
            raise DrillError("single-link replay executable copy failed hash verification")
        return {"bytes": total, "sha256": copied_sha256, "source_hardlinks": source_stat.st_nlink}
    except (OSError, sealed_guard.SealedError) as exc:
        if created_copy:
            destination.unlink(missing_ok=True)
        raise DrillError(f"cold Cargo executable copy refused: {exc}") from exc


def reimport_bundle(bundle: Path, destination: Path) -> dict[str, object]:
    parsed, manifest = load_bundle(bundle)
    destination = require_private_path(destination, new=True)
    files = verify_bundle_files(bundle, parsed)
    os.mkdir(destination, 0o700)
    try:
        for rel, payload in sorted(files.items()):
            path = destination / rel
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            write_private(path, payload)
        write_private(destination / "PORTABILITY.json", manifest)
        write_private(destination / "COMPLETE", (sha256(manifest) + "\n").encode("ascii"))
    except BaseException:
        shutil.rmtree(destination, ignore_errors=True)
        raise
    return {"bundle_sha256": sha256(manifest), "files": len(files), "bytes": sum(map(len, files.values()))}


def run_replay(repo: Path, binary: Path, bundle: Path, reimport_dir: Path, output: Path,
               runtime_env: dict[str, str] | None = None) -> dict[str, object]:
    repo = guarded_existing_path(repo, "replay source checkout")
    binary = guarded_existing_path(binary, "replay executable")
    bundle = require_private_path(bundle)
    reimport_dir = require_private_path(reimport_dir, new=True)
    output = require_private_path(output, new=True)
    if not repo.is_dir() or not binary.is_file() or not os.access(binary, os.X_OK):
        raise DrillError("replay source must be a directory and binary must be executable")
    parsed, _manifest = load_bundle(bundle)
    if repo not in binary.parents:
        raise DrillError("replay binary must be beneath the fresh clone")
    require_disjoint_paths(
        (("replay source", repo), ("bundle", bundle), ("reimport output", reimport_dir), ("replay output", output))
    )
    files = verify_bundle_files(bundle, parsed)
    reimport_bundle(bundle, reimport_dir)
    try:
        head = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True, timeout=10).stdout.strip()
        status = subprocess.run(["git", "-C", str(repo), "status", "--porcelain=v1"], check=True,
                                capture_output=True, text=True, timeout=10).stdout
    except (OSError, subprocess.SubprocessError) as exc:
        raise DrillError("cannot verify fresh clone state") from exc
    if head != parsed.get("source_commit") or status:
        raise DrillError("fresh clone is not clean at the pinned source commit")
    output = require_private_path(output, new=True)
    settings_expected = files["replay/settings.tsv"]
    profile_id, profile_cli = regional_run_profile(settings_expected)
    os.mkdir(output, 0o700)
    input_path = reimport_dir / "recording/normalized.tsv"
    snapshot_path = reimport_dir / "adaptation/airspace.snapshot"
    executable_sha256_before = sha256(read_bounded(binary, 512 * 1024 * 1024))
    try:
        env = {key: os.environ[key] for key in ("PATH", "HOME", "RUSTUP_HOME", "CARGO_HOME", "TMPDIR") if key in os.environ}
        if runtime_env is not None:
            env.update({key: runtime_env[key] for key in ("CARGO_HOME", "CARGO_TARGET_DIR") if key in runtime_env})
        command = [str(binary), "run", "--input", str(input_path), "--snapshot", str(snapshot_path), "--out", str(output / "run")]
        if profile_cli is not None:
            command.extend(["--profile", profile_cli])
        result = subprocess.run(command,
                                cwd=repo, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=14_400, check=False)
    except (OSError, subprocess.SubprocessError) as exc:
        raise DrillError("fresh-clone replay could not be completed") from exc
    if result.returncode != 0:
        raise DrillError(f"fresh-clone replay exited {result.returncode}")
    executable_sha256_after = sha256(read_bounded(binary, 512 * 1024 * 1024))
    if executable_sha256_after != executable_sha256_before:
        raise DrillError("replay executable changed while running")
    run_dir = output / "run"
    actual_manifest, actual_complete = verify_complete(run_dir, "manifest.tsv", "COMPLETE", MAX_RUN_MANIFEST_BYTES)
    actual_fields, actual_frames = parse_regional_manifest(actual_manifest)
    verify_frame_files(run_dir, actual_frames)
    actual_settings = read_bounded(run_dir / "settings.tsv", MAX_SETTINGS_BYTES)
    expected_fields, expected_frames = parse_regional_manifest(files["replay/manifest.tsv"])
    if actual_settings != settings_expected or actual_fields["settings_sha256"] != sha256(actual_settings):
        raise DrillError("fresh-clone replay settings differ from the exported configuration")
    stable_keys = ("input_sha256", "snapshot_sha256", "settings_sha256")
    if any(actual_fields[key] != expected_fields[key] for key in stable_keys) or actual_frames != expected_frames:
        raise DrillError("fresh-clone replay input, adaptation, settings, or frame digests differ")
    require_executable_digest_matches(actual_fields, executable_sha256_before)
    if actual_fields["input_sha256"] != sha256(files["recording/normalized.tsv"]):
        raise DrillError("fresh-clone replay input digest does not match the recording index")
    if actual_fields["snapshot_sha256"] != sha256(files["adaptation/airspace.snapshot"]):
        raise DrillError("fresh-clone replay adaptation digest differs from the exported snapshot")
    return {
        "status": "MATCH",
        "regional_run_profile": profile_id,
        "bundle_sha256": sha256(_manifest),
        "recording_index_sha256": parsed["recording_index_sha256"],
        "baseline_manifest_sha256": parsed["baseline_manifest_sha256"],
        "replay_manifest_sha256": actual_complete,
        "input_sha256": actual_fields["input_sha256"],
        "adaptation_sha256": actual_fields["snapshot_sha256"],
        "settings_sha256": actual_fields["settings_sha256"],
        "frame_count": len(actual_frames),
        "frame_digest_set_sha256": sha256("\n".join("\t".join(row) for row in actual_frames).encode("utf-8")),
        "baseline_executable_sha256": expected_fields["executable_sha256"],
        "executed_executable_sha256": executable_sha256_before,
    }


def cold_portability_drill(source_repo: Path, bundle: Path, clone_out: Path, cargo_home: Path,
                           reimport_out: Path, output: Path) -> dict[str, object]:
    source_repo = guarded_existing_path(source_repo, "source checkout")
    if not source_repo.is_dir():
        raise DrillError("source checkout must be a directory")
    bundle = require_private_path(bundle)
    clone_out = require_private_path(clone_out, new=True)
    cargo_home = private_directory_candidate(cargo_home)
    reimport_out = require_private_path(reimport_out, new=True)
    output = require_private_path(output, new=True)
    require_disjoint_paths(
        (
            ("source checkout", source_repo),
            ("bundle", bundle),
            ("fresh clone", clone_out),
            ("Cargo home", cargo_home),
            ("reimport output", reimport_out),
            ("result output", output),
        )
    )
    source_repo, source_commit = source_checkout(source_repo)
    parsed, _ = load_bundle(bundle)
    if parsed.get("source_commit") != source_commit:
        raise DrillError("source checkout does not match the exported bundle commit")
    cargo_home = make_empty_private_dir(cargo_home)
    if cargo_home.exists() and any(cargo_home.iterdir()):
        raise DrillError("Cargo home is not empty before the fresh-clone build")
    try:
        subprocess.run(["git", "clone", "--no-local", "--no-hardlinks", "--quiet",
                        str(source_repo), str(clone_out)], check=True, timeout=600,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run_git(clone_out, "checkout", "--detach", source_commit)
    except (OSError, subprocess.SubprocessError, DrillError) as exc:
        shutil.rmtree(clone_out, ignore_errors=True)
        raise DrillError("fresh source clone could not be created") from exc
    if run_git(clone_out, "rev-parse", "HEAD") != source_commit or run_git(
            clone_out, "status", "--porcelain=v1", "--untracked-files=all"):
        raise DrillError("fresh clone is not clean at the bound source commit")
    target_dir = clone_out / "target"
    if target_dir.exists():
        if not target_dir.is_dir() or any(target_dir.iterdir()):
            raise DrillError("fresh clone target directory is not empty before build")
    else:
        target_dir.mkdir(mode=0o700)
    if any(cargo_home.iterdir()):
        raise DrillError("Cargo home became nonempty before build submission")
    env = {
        key: os.environ[key]
        for key in ("PATH", "HOME", "RUSTUP_HOME", "TMPDIR")
        if key in os.environ
    }
    if "PATH" not in env:
        raise DrillError("PATH is unavailable in the guarded build environment")
    env["CARGO_HOME"] = str(cargo_home)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO_BUILD_JOBS"] = "2"
    env["RUSTUP_TOOLCHAIN"] = RUST_TOOLCHAIN
    rustup_home = Path(env.get("RUSTUP_HOME", str(Path(env.get("HOME", "")) / ".rustup")))
    env["RUSTUP_HOME"] = str(rustup_home)
    for key in ("HOME", "RUSTUP_HOME", "TMPDIR"):
        if key in env:
            try:
                GUARD.check(env[key])
            except sealed_guard.SealedError as exc:
                raise DrillError(f"guarded build environment {key} refusal: {exc}") from exc
    cargo = shutil.which("cargo", path=env.get("PATH"))
    rustc = shutil.which("rustc", path=env.get("PATH"))
    if cargo is None:
        raise DrillError("cargo executable is unavailable in the guarded environment")
    if rustc is None:
        raise DrillError("rustc executable is unavailable in the guarded environment")
    toolchains = rustup_home / "toolchains"
    try:
        GUARD.check(toolchains)
    except sealed_guard.SealedError as exc:
        raise DrillError(f"guarded Rust toolchain directory refusal: {exc}") from exc
    if not toolchains.is_dir() or not any(path.name.startswith(f"{RUST_TOOLCHAIN}-") for path in toolchains.iterdir()):
        raise DrillError(f"pinned Rust toolchain {RUST_TOOLCHAIN} is not preinstalled")
    for label, executable in (("cargo", cargo), ("rustc", rustc)):
        try:
            GUARD.check(executable)
        except sealed_guard.SealedError as exc:
            raise DrillError(f"guarded {label} executable refusal: {exc}") from exc
    try:
        cargo_version = subprocess.run(
            [cargo, f"+{RUST_TOOLCHAIN}", "--version"], check=True, capture_output=True,
            text=True, env=env, stdin=subprocess.DEVNULL, timeout=30,
        ).stdout.strip()
        rustc_version = subprocess.run(
            [rustc, "--version", "--verbose"], check=True,
            capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL, timeout=30,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError) as exc:
        raise DrillError(f"pinned Rust toolchain {RUST_TOOLCHAIN} is unavailable") from exc
    target_triple = next(
        (line.partition(":")[2].strip() for line in rustc_version.splitlines() if line.startswith("host:")),
        None,
    )
    if not target_triple:
        raise DrillError("pinned rustc did not report its host target")
    if any(cargo_home.iterdir()) or any(target_dir.iterdir()):
        raise DrillError("toolchain identification changed the empty cold-build caches")
    build_command = [cargo, f"+{RUST_TOOLCHAIN}", "build", "--locked", "--offline",
                     "-p", "oa-regional-session", "--bin", "oa-regional-session",
                     "--message-format=json-render-diagnostics"]
    try:
        build = subprocess.run(build_command, cwd=clone_out, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=3600, check=False)
    except (OSError, subprocess.SubprocessError) as exc:
        raise DrillError("guarded empty-cache fresh-clone build could not complete") from exc
    if build.returncode != 0:
        raise DrillError(f"guarded empty-cache fresh-clone build failed with exit {build.returncode}")
    if not any(cargo_home.iterdir()) or not any(target_dir.iterdir()):
        raise DrillError("guarded build did not populate the isolated empty Cargo home and target")
    cargo_binary = cold_cargo_executable(build.stdout, clone_out, target_dir)
    if not cargo_binary.is_file() or not os.access(cargo_binary, os.X_OK):
        raise DrillError("guarded build did not produce the expected regional-session executable")
    copy_receipt = copy_fresh_cargo_executable(
        cargo_binary,
        target_dir / "debug" / ".j2-replay" / "oa-regional-session",
        target_dir / "debug" / "oa-regional-session",
        target_dir,
    )
    os.mkdir(output, 0o700)
    replay_binary = target_dir / "debug" / ".j2-replay" / "oa-regional-session"
    replay_result = run_replay(clone_out, replay_binary, bundle, reimport_out, output / "replay", env)
    cargo_entries, cargo_tree_sha = directory_digest(cargo_home)
    target_entries, target_tree_sha = directory_digest(target_dir)
    report = {
        "schema": "openatca-j2-cold-replay-result-v1",
        "status": replay_result["status"],
        "regional_run_profile": replay_result["regional_run_profile"],
        "source_commit": source_commit,
        "bundle_sha256": replay_result["bundle_sha256"],
        "recording_index_sha256": replay_result["recording_index_sha256"],
        "baseline_manifest_sha256": replay_result["baseline_manifest_sha256"],
        "replay_manifest_sha256": replay_result["replay_manifest_sha256"],
        "input_sha256": replay_result["input_sha256"],
        "adaptation_sha256": replay_result["adaptation_sha256"],
        "settings_sha256": replay_result["settings_sha256"],
        "frame_count": replay_result["frame_count"],
        "frame_digest_set_sha256": replay_result["frame_digest_set_sha256"],
        "baseline_executable_sha256": replay_result["baseline_executable_sha256"],
        "executed_executable_sha256": replay_result["executed_executable_sha256"],
        "cargo_executable_copy_bytes": copy_receipt["bytes"],
        "cargo_executable_copy_sha256": copy_receipt["sha256"],
        "cargo_executable_source_hardlinks": copy_receipt["source_hardlinks"],
        "cargo_home_empty_before_build": True,
        "target_empty_before_build": True,
        "cargo_home_entry_count_after_build": cargo_entries,
        "cargo_home_tree_sha256_after_build": cargo_tree_sha,
        "target_entry_count_after_build": target_entries,
        "target_tree_sha256_after_build": target_tree_sha,
        "build_command": ["cargo", f"+{RUST_TOOLCHAIN}", "build", "--locked", "--offline",
                          "-p", "oa-regional-session", "--bin", "oa-regional-session",
                          "--message-format=json-render-diagnostics"],
        "cargo_profile": "dev-default",
        "feature_selection": "default features; no explicit --features flags",
        "cargo_version": cargo_version,
        "rustc_version_verbose": rustc_version,
        "target_triple": target_triple,
        "platform_system": platform.system(),
        "platform_release": platform.release(),
        "platform_machine": platform.machine(),
        "build_environment_keys": sorted(env),
        "build_environment_sha256": sha256(json.dumps(env, sort_keys=True, separators=(",", ":")).encode()),
        "cargo_stdout_sha256": sha256(build.stdout),
        "cargo_stderr_sha256": sha256(build.stderr),
    }
    result_bytes = (json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n").encode()
    write_private(output / "RESULT.json", result_bytes)
    write_private(output / "COMPLETE", (sha256(result_bytes) + "\n").encode("ascii"))
    return {"status": report["status"], "source_commit": source_commit,
            "result_sha256": sha256(result_bytes), "frame_count": report["frame_count"],
            "frame_digest_set_sha256": report["frame_digest_set_sha256"]}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    export = commands.add_parser("export")
    export.add_argument("--recording-import", type=Path, required=True)
    export.add_argument("--adaptation-package", type=Path, required=True)
    export.add_argument("--baseline-run", type=Path, required=True)
    export.add_argument("--source-commit", required=True)
    export.add_argument("--out", type=Path, required=True)
    reimport = commands.add_parser("reimport")
    reimport.add_argument("--bundle", type=Path, required=True)
    reimport.add_argument("--out", type=Path, required=True)
    replay = commands.add_parser("replay")
    replay.add_argument("--repo", type=Path, required=True)
    replay.add_argument("--binary", type=Path, required=True)
    replay.add_argument("--bundle", type=Path, required=True)
    replay.add_argument("--reimport-out", type=Path, required=True)
    replay.add_argument("--out", type=Path, required=True)
    drill = commands.add_parser("cold-drill")
    drill.add_argument("--source-repo", type=Path, required=True)
    drill.add_argument("--bundle", type=Path, required=True)
    drill.add_argument("--clone-out", type=Path, required=True)
    drill.add_argument("--cargo-home", type=Path, required=True)
    drill.add_argument("--reimport-out", type=Path, required=True)
    drill.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "export":
            result = export_bundle(args.recording_import, args.adaptation_package,
                                  args.baseline_run, args.source_commit, args.out)
        elif args.command == "reimport":
            result = reimport_bundle(args.bundle, args.out)
        elif args.command == "cold-drill":
            result = cold_portability_drill(args.source_repo, args.bundle, args.clone_out,
                                            args.cargo_home, args.reimport_out, args.out)
        else:
            result = run_replay(args.repo, args.binary, args.bundle, args.reimport_out,
                                args.out)
    except (DrillError, OSError, ValueError, TypeError, KeyError) as exc:
        print(f"j2-portability: refused: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
