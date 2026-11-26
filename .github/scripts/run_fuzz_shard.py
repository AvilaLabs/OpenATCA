#!/usr/bin/env python3
"""Validate cargo-fuzz's target listing and run one deterministic CI shard."""
from __future__ import annotations

import argparse
import os
import re
import stat
import subprocess
import sys
import tomllib
from pathlib import Path

SHARD_COUNT = 5
MAX_TARGETS = 256
MAX_MANIFEST_BYTES = 64 * 1024
MAX_TARGET_LIST_BYTES = 16 * 1024
# Current inventory is at most 78 seeds per target, 537 total, with an 11,359-byte largest
# seed. These fixed limits allow >60% count growth, >44% per-file growth, and a 1 MiB
# per-target byte budget (>18% over 78 * 11,359 bytes) before requiring reviewed adjustment.
MAX_SEEDS_PER_TARGET = 128
MAX_SEED_FILE_BYTES = 16 * 1024
MAX_SEED_BYTES_PER_TARGET = 1024 * 1024
TARGET_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*\Z")
RSS_LIMIT_MB = 2048
MAX_TOTAL_TIME_SECONDS = 60


class FuzzShardError(ValueError):
    """Invalid inventory, listing, or shard request."""


def _bounded_bytes(path: Path, maximum: int, label: str) -> bytes:
    try:
        before = path.lstat()
    except OSError as exc:
        raise FuzzShardError(f"cannot stat {label}: {exc}") from exc
    if not stat.S_ISREG(before.st_mode):
        raise FuzzShardError(f"{label} is not a regular file")
    if before.st_size > maximum:
        raise FuzzShardError(f"{label} exceeds {maximum} bytes")
    try:
        with path.open("rb") as stream:
            opened = os.fstat(stream.fileno())
            if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino, opened.st_size) != (before.st_dev, before.st_ino, before.st_size):
                raise FuzzShardError(f"{label} changed while opening")
            data = stream.read(maximum + 1)
    except FuzzShardError:
        raise
    except OSError as exc:
        raise FuzzShardError(f"cannot read {label}: {exc}") from exc
    if len(data) > maximum:
        raise FuzzShardError(f"{label} exceeds {maximum} bytes")
    if len(data) != opened.st_size:
        raise FuzzShardError(f"{label} changed during bounded read")
    return data


def cargo_bin_inventory(manifest: Path) -> list[str]:
    """Read and validate the complete explicit [[bin]] target set."""
    try:
        data = _bounded_bytes(manifest, MAX_MANIFEST_BYTES, "fuzz Cargo.toml")
        config = tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise FuzzShardError(f"invalid fuzz Cargo.toml: {exc}") from exc
    entries = config.get("bin")
    if not isinstance(entries, list) or not 1 <= len(entries) <= MAX_TARGETS:
        raise FuzzShardError("fuzz Cargo.toml must declare 1..256 [[bin]] targets")
    names: list[str] = []
    for entry in entries:
        name = entry.get("name") if isinstance(entry, dict) else None
        if not isinstance(name, str) or not TARGET_RE.fullmatch(name):
            raise FuzzShardError("every [[bin]] target needs a valid literal name")
        names.append(name)
    if len(set(names)) != len(names):
        raise FuzzShardError("fuzz Cargo.toml contains duplicate [[bin]] names")
    return sorted(names)


def cargo_fuzz_listing(payload: bytes, declared_targets: list[str]) -> list[str]:
    """Require cargo-fuzz output to be a nonempty exact, unique inventory match."""
    if not payload or len(payload) > MAX_TARGET_LIST_BYTES:
        raise FuzzShardError("cargo-fuzz target listing is empty or exceeds its byte cap")
    try:
        text = payload.decode("utf-8", errors="strict")
    except UnicodeDecodeError as exc:
        raise FuzzShardError("cargo-fuzz target listing is not UTF-8") from exc
    names = [line.strip() for line in text.splitlines()]
    if not names or any(not name or not TARGET_RE.fullmatch(name) for name in names):
        raise FuzzShardError("cargo-fuzz listing contains an empty or invalid target name")
    if len(names) > MAX_TARGETS:
        raise FuzzShardError("cargo-fuzz target listing exceeds 256 rows")
    if len(set(names)) != len(names):
        raise FuzzShardError("cargo-fuzz target listing contains duplicate names")
    expected = set(declared_targets)
    actual = set(names)
    if actual != expected or len(names) != len(declared_targets):
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise FuzzShardError(f"cargo-fuzz inventory mismatch; missing={missing}, extra={extra}")
    return sorted(names)


def targets_for_shard(targets: list[str], shard: int, shard_count: int = SHARD_COUNT) -> list[str]:
    """Assign sorted target names by index modulo shard_count, with no overlap."""
    if shard_count != SHARD_COUNT or not 0 <= shard < SHARD_COUNT:
        raise FuzzShardError("shard index/count must be one of 0..4 / 5")
    ordered = sorted(targets)
    if not ordered or len(ordered) > MAX_TARGETS or len(set(ordered)) != len(ordered):
        raise FuzzShardError("target inventory must be nonempty, unique, and bounded")
    return [name for index, name in enumerate(ordered) if index % SHARD_COUNT == shard]


def validate_expected_targets(targets: list[str], shard: int, expected_csv: str) -> None:
    """Require explicit workflow membership to match the real, validated target inventory."""
    if not 0 <= shard < SHARD_COUNT:
        raise FuzzShardError("shard index must be one of 0..4 / 5")
    expected = expected_csv.split(",")
    if not expected or any(not TARGET_RE.fullmatch(name) for name in expected):
        raise FuzzShardError("workflow shard target list is empty or contains an invalid name")
    if len(expected) != len(set(expected)):
        raise FuzzShardError("workflow shard target list contains duplicates")
    actual_group = targets_for_shard(targets, shard)
    if expected != actual_group:
        raise FuzzShardError(
            f"workflow shard {shard} differs from computed membership; "
            f"expected={expected}, actual={actual_group}"
        )


def validate_shard_union(targets: list[str]) -> None:
    """Prove the fixed shard plan covers the validated runtime inventory exactly once."""
    shards = [targets_for_shard(targets, index) for index in range(SHARD_COUNT)]
    flattened = [target for group in shards for target in group]
    if sorted(flattened) != sorted(targets) or len(flattened) != len(set(flattened)):
        raise FuzzShardError("five-shard partition does not cover every listed target exactly once")



def copy_seeds_bounded(seed_dir: Path, corpus: Path) -> int:
    """Copy the complete seed directory after bounded metadata preflight."""
    try:
        directory = seed_dir.lstat()
    except FileNotFoundError:
        return 0
    except OSError as exc:
        raise FuzzShardError(f"cannot stat seed directory: {exc}") from exc
    if not stat.S_ISDIR(directory.st_mode):
        raise FuzzShardError(f"seed path is not a regular directory: {seed_dir}")

    entries: list[tuple[Path, os.stat_result]] = []
    total_bytes = 0
    try:
        with os.scandir(seed_dir) as scan:
            for entry in scan:
                if len(entries) >= MAX_SEEDS_PER_TARGET:
                    raise FuzzShardError(f"seed count exceeds {MAX_SEEDS_PER_TARGET}: {seed_dir}")
                info = entry.stat(follow_symlinks=False)
                if not stat.S_ISREG(info.st_mode):
                    raise FuzzShardError(f"seed entry is not a regular file: {entry.name}")
                if info.st_size > MAX_SEED_FILE_BYTES:
                    raise FuzzShardError(f"seed file exceeds {MAX_SEED_FILE_BYTES} bytes: {entry.name}")
                total_bytes += info.st_size
                if total_bytes > MAX_SEED_BYTES_PER_TARGET:
                    raise FuzzShardError(f"seed bytes exceed {MAX_SEED_BYTES_PER_TARGET}: {seed_dir}")
                entries.append((Path(entry.path), info))
    except FuzzShardError:
        raise
    except OSError as exc:
        raise FuzzShardError(f"cannot enumerate seeds: {exc}") from exc

    entries.sort(key=lambda item: item[0].name)
    copied = 0
    for seed, expected in entries:
        try:
            with seed.open("rb") as stream:
                opened = os.fstat(stream.fileno())
                if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino, opened.st_size) != (expected.st_dev, expected.st_ino, expected.st_size):
                    raise FuzzShardError(f"seed changed after preflight: {seed.name}")
                payload = stream.read(MAX_SEED_FILE_BYTES + 1)
        except FuzzShardError:
            raise
        except OSError as exc:
            raise FuzzShardError(f"cannot read seed {seed.name}: {exc}") from exc
        if len(payload) != expected.st_size or len(payload) > MAX_SEED_FILE_BYTES:
            raise FuzzShardError(f"seed changed or exceeded its read cap: {seed.name}")
        (corpus / seed.name).write_bytes(payload)
        copied += 1
    return copied

def run_shard(targets: list[str], shard: int, repo_root: Path, runner_temp: Path) -> int:
    fuzz_root = repo_root / "fuzz"
    artifacts_root = fuzz_root / "artifacts" / f"shard-{shard}"
    artifacts_root.mkdir(parents=True, exist_ok=True)
    for target in targets_for_shard(targets, shard):
        corpus = runner_temp / "fuzz-corpus" / target
        corpus.mkdir(parents=True, exist_ok=True)
        seed_dir = fuzz_root / "seeds" / target
        copy_seeds_bounded(seed_dir, corpus)
        artifact_dir = artifacts_root / target
        artifact_dir.mkdir(parents=True, exist_ok=True)
        command = [
            "cargo", "+nightly-2026-08-21", "fuzz", "run", target, str(corpus), "--",
            f"-artifact_prefix={artifact_dir.resolve()}/",
            f"-rss_limit_mb={RSS_LIMIT_MB}",
            f"-max_total_time={MAX_TOTAL_TIME_SECONDS}",
        ]
        print(f"::group::fuzz shard {shard}/{SHARD_COUNT}: {target}", flush=True)
        result = subprocess.run(command, cwd=fuzz_root, check=False)
        print("::endgroup::", flush=True)
        if result.returncode != 0:
            return result.returncode
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--targets-file", required=True, type=Path)
    parser.add_argument("--shard", required=True, type=int)
    parser.add_argument("--expected-targets", required=True)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args(argv)
    try:
        targets = cargo_fuzz_listing(
            _bounded_bytes(args.targets_file, MAX_TARGET_LIST_BYTES, "cargo-fuzz target listing"),
            cargo_bin_inventory(args.repo_root / "fuzz" / "Cargo.toml"),
        )
        validate_shard_union(targets)
        validate_expected_targets(targets, args.shard, args.expected_targets)
        runner_temp = Path(os.environ["RUNNER_TEMP"])
        return run_shard(targets, args.shard, args.repo_root, runner_temp)
    except (FuzzShardError, KeyError, OSError) as exc:
        print(f"fuzz shard refused: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
