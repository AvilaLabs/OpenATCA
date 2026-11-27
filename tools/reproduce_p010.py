#!/usr/bin/env python3
"""Run the P-010 public ADS-B arm, local raw-SFDPS comparison, or aggregate capture comparison.

This is a narrow, offline coordinator. It accepts no shell fragments, downloads nothing, never
opens an environment file, and preserves failed runs as INCOMPLETE. The exact manifest schema and
source/binary binding contract are frozen with this source revision.
"""
from __future__ import annotations

import argparse
import csv
import functools
import hashlib
import importlib.util
import io
import json
import math
import os
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

SCRIPT_PATH = Path(__file__).resolve(strict=True)
TOOLS_DIR = SCRIPT_PATH.parent
ROOT = TOOLS_DIR.parent
GUARD_SPEC = importlib.util.spec_from_file_location("p010_sealed_guard", TOOLS_DIR / "sealed_guard.py")
if GUARD_SPEC is None or GUARD_SPEC.loader is None:
    raise RuntimeError("cannot load the existing sealed path guard")
sealed_guard = importlib.util.module_from_spec(GUARD_SPEC)
GUARD_SPEC.loader.exec_module(sealed_guard)

SCHEMA = "openatca-p010-manifest-v3"
REPORT_SCHEMA = "openatca-p010-run-report-v2"
PUBLIC_SCHEMA = "openatca-p010-public-summary-v1"
AGGREGATE_SCHEMA = "openatca-p010-aggregate-v3"
CROSS_CAPTURE_SCHEMA = "openatca-p010-cross-capture-comparison-v1"
DAY = "2026-11-30"
ARCHIVE_FROM = "2026-11-30T00:00:00Z"
ARCHIVE_TO = "2026-12-01T00:00:00Z"
WARMUP_FROM = "2026-11-30T05:00:00Z"
SCORE_FROM = "2026-11-30T06:00:00Z"
SCORE_TO = "2026-12-01T00:00:00Z"
ZAU_BOX = [38.5, 46.5, -93.5, -83.5]
TIMING_PROFILES = ("legacy-day-intervals", "fixed-10-30-120-v1")
GRID_PROFILES = (None, "sfdps-v1")

# P-010 v1 ceilings. They are manifest-pinned and must be frozen before any run; callers cannot
# raise them. The 64 GiB archive ceilings match the established reproduction package limits.
LIMITS = {
    "max_archive_parts": 16,
    "max_part_bytes": 8 * 1024**3,
    "max_archive_bytes": 64 * 1024**3,
    "max_sfdps_files": 1_000_000,
    "max_sfdps_directories": 100_000,
    "max_sfdps_bytes": 64 * 1024**3,
    "max_manifest_bytes": 64 * 1024,
    "max_log_bytes": 16 * 1024**2,
    "max_output_file_bytes": 2 * 1024**3,
    "max_output_rows": 20_000_000,
    "max_child_output_bytes": 4 * 1024**3,
    "max_output_files": 4096,
    "max_output_directories": 4096,
    "max_output_depth": 64,
    "max_wall_seconds": 14_400,
    "min_free_bytes": 64 * 1024**3,
}
BUILD_ARGV = {
    "clipper": ["build", "--release", "--locked", "--offline", "-p", "oa-adsb", "--bin", "oa-adsb-clip", "--message-format=json-render-diagnostics"],
    "candidate": ["build", "--release", "--locked", "--offline", "-p", "oa-track-tools", "--bin", "oa-track-run", "--message-format=json-render-diagnostics"],
    "candidate_scorer": ["build", "--release", "--locked", "--offline", "-p", "oa-track-tools", "--example", "p002", "--message-format=json-render-diagnostics"],
    "replication_scorer": ["build", "--release", "--locked", "--offline", "-p", "oa-track", "--example", "p002", "--message-format=json-render-diagnostics"],
}


def rustup_tool(tool: str) -> str:
    rustup = shutil.which("rustup")
    if rustup is None:
        raise RuntimeError("rustup is required to locate the pinned Rust 1.89.0 toolchain")
    located = subprocess.run(
        [rustup, "which", "--toolchain", "1.89.0", tool],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    ).stdout.strip()
    resolved = Path(located).resolve(strict=True)
    if not resolved.is_file():
        raise RuntimeError("rustup returned a non-file tool path")
    return os.fspath(resolved)


TOOLCHAIN_TRUST_ASSUMPTION = (
    "trusted installed Rust 1.89.0 paths; receipt hashes and version probes establish process-time "
    "identity only, not secure-boot or host attestation"
)


@functools.lru_cache(maxsize=1)
def rust_toolchain() -> tuple[str, str, str, str]:
    cargo = rustup_tool("cargo")
    rustc = rustup_tool("rustc")
    root = os.fspath(Path(cargo).parent)
    return cargo, rustc, root, os.pathsep.join((root, "/usr/bin", "/bin"))
ARTIFACT_NAMES = {"clipper": "oa-adsb-clip", "candidate": "oa-track-run", "candidate_scorer": "p002", "replication_scorer": "p002"}
REPLICATION_TRACKER_COMMIT = "4ad16fe19203c165ff9b8ecf477005ab1fd50384"
# The C derivative adds only the scorer example and its ERAM export parser module registration;
# it may not modify the tracker core, timing configuration, replay, or sealed-input policy.
SCORER_PATCH_PATHS = ["crates/oa-bench/src/eram_export.rs", "crates/oa-bench/src/lib.rs", "crates/oa-track/examples/p002.rs"]
HEX64 = re.compile(r"[0-9a-f]{64}\Z", re.ASCII)
HEX40 = re.compile(r"[0-9a-f]{40}\Z", re.ASCII)
SAFE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,254}\Z", re.ASCII)
ARM_NAME = re.compile(r"[a-z][a-z0-9_-]{0,31}\Z", re.ASCII)
MAX_LINE_BYTES = 64 * 1024
MAX_DEPTH = 64
# C7: directory traversal is bounded independently of regular file/byte counts.
CHILD_ENV = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "TZ": "UTC", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0"}

# This source-only probe is compiled by the frozen build procedure against each exact Cargo
# oa-sealed/oa-sha256 artifact pair. At run time it reads no input file: only absent synthetic
# path names and in-memory timestamps are passed to the compiled production guard.
GUARD_PROBE_SOURCE = r'''use std::path::Path;

fn require(ok: bool, reason: &str) {
    if !ok { eprintln!("P010_GUARD_PROBE_REFUSED: {reason}"); std::process::exit(2); }
}

fn main() {
    let embedded = oa_sealed::Sealed::production_embedded();
    let production = oa_sealed::Sealed::production();
    let list_hash = oa_sha256::to_hex(&oa_sha256::sha256(oa_sealed::LIST.as_bytes()));
    let allowed = Path::new("/tmp/p010-guard-probe-2026-11-30/absent-row");
    let refused = Path::new("/tmp/p010-guard-probe-2026-11-20/absent-row");
    for guard in [embedded, production] {
        require(guard.check(allowed).is_ok(), "synthetic unsealed path refused");
        require(guard.check(refused).is_err(), "synthetic sealed date path admitted");
        require(guard.in_window(1_795_132_800_i64 * 1_000_000_000), "synthetic sealed timestamp admitted");
        require(!guard.in_window(1_795_996_800_i64 * 1_000_000_000), "synthetic unsealed timestamp refused");
    }
    let open_error = match oa_sealed::open(refused) {
        Ok(_) => { eprintln!("P010_GUARD_PROBE_REFUSED: sealed synthetic sentinel unexpectedly opened"); std::process::exit(2); },
        Err(error) => error,
    };
    require(open_error.to_string().contains("OA-SEAL-001"), "sealed opener did not refuse before file access");
    println!("P010_GUARD_PROBE_OK v1 list_sha256={list_hash} paths=4 timestamps=4 opens=1");
}
'''
GUARD_PROBE_SHA256 = hashlib.sha256(GUARD_PROBE_SOURCE.encode("utf-8")).hexdigest()
BUILD_RECEIPT_SCHEMA = "p010-build-v3"
BUILD_ROLES = ("clipper", "candidate", "candidate_scorer", "replication_scorer")
GUARD_BUILD_ROLES = BUILD_ROLES
MAX_PROBE_BYTES = 1024
MAX_BUILD_RECEIPT_BYTES = 4 * 1024 * 1024
MAX_LOCAL_ARTIFACTS = 4096
MAX_LOCAL_ARTIFACT_BYTES = 8 * 1024**3
MAX_BUILD_TARGET_FILES = 100_000
MAX_BUILD_TARGET_DIRECTORIES = 10_000
MAX_BUILD_TARGET_BYTES = 16 * 1024**3
MAX_BUILD_TARGET_DEPTH = 64


class Refused(Exception):
    """Invalid or unsafe input; no run should be labelled complete."""


class Incomplete(Exception):
    """A child or output failed after the new evidence directory was created."""


def _guard() -> Any:
    return sealed_guard.production()


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _exact_keys(value: Any, required: set[str], where: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != required:
        raise Refused(f"{where}: keys must be exactly {sorted(required)}")
    return value


def _object_no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise ValueError(f"duplicate JSON key: {key}")
        out[key] = value
    return out


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-JSON numeric constant {value}")


def _validate_compare_pairs(arms: Any) -> None:
    if not isinstance(arms, list) or len(arms) not in {2, 4}:
        raise Refused("compare_arms must contain one matched candidate/C pair per selected publication grid")
    by_grid: dict[str | None, set[str]] = {}
    for arm in arms:
        grid = arm["publication_grid"]
        roles = by_grid.setdefault(grid, set())
        role = arm["tracker_role"]
        if role in roles:
            raise Refused("compare_arms has duplicate source-role arms for one publication grid")
        roles.add(role)
    if any(roles != {"candidate", "replication"} for roles in by_grid.values()):
        raise Refused("each publication grid must have exactly one candidate arm and one frozen-C replication arm")


def _canonical_build_path(value: Any, where: str) -> Path:
    if not isinstance(value, str) or not Path(value).is_absolute():
        raise Refused(f"{where} must be an absolute canonical path")
    path = Path(value)
    _guard().check(path)
    if path.resolve(strict=False) != path:
        raise Refused(f"{where} must not contain unresolved aliases or symlink components")
    if path == Path("/") or len(path.parts) < 3:
        raise Refused(f"{where} must name a dedicated child directory")
    return path


def _build_argv(role: str, target_dir: Path) -> list[str]:
    cargo, _, _, _ = rust_toolchain()
    base = [cargo, *BUILD_ARGV[role]]
    marker = "--message-format=json-render-diagnostics"
    if marker not in base:
        raise Refused(f"internal build argv is missing Cargo JSON output for {role}")
    return [*base[:-1], "--target-dir", str(target_dir), "--config", "build.incremental=false",
            "--config", 'build.rustc-wrapper=""', "--config", 'build.rustc-workspace-wrapper=""', base[-1]]


def _path_is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _read_guarded(path: Path, cap: int) -> bytes:
    if cap < 0:
        raise Refused("negative read cap")
    with _guard().open(path, "rb") as f:
        st = os.fstat(f.fileno())
        if not stat.S_ISREG(st.st_mode):
            raise Refused(f"{path}: expected a regular file")
        if st.st_size > cap:
            raise Refused(f"{path}: exceeds the {cap}-byte limit")
        data = f.read(cap + 1)
    if len(data) > cap:
        raise Refused(f"{path}: exceeds the {cap}-byte limit")
    return data


def _open_guarded(path: Path):
    return _guard().open(path, "rb")


def _hash_file(path: Path, cap: int) -> tuple[int, str]:
    h = hashlib.sha256()
    size = 0
    with _open_guarded(path) as f:
        before = os.fstat(f.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > cap:
            raise Refused(f"{path}: unsafe or oversized hashed file")
        while True:
            block = f.read(min(1024 * 1024, cap + 1 - size))
            if not block:
                break
            size += len(block)
            if size > cap:
                raise Refused(f"{path}: exceeds the {cap}-byte limit")
            h.update(block)
        after = os.fstat(f.fileno())
    named = path.lstat()
    identity = lambda st: (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns)
    if identity(before) != identity(after) or identity(after) != identity(named) or size != after.st_size:
        raise Refused(f"{path}: changed during hashing")
    return size, h.hexdigest()


def _safe_basename(value: Any, where: str) -> str:
    if not isinstance(value, str) or not SAFE_NAME.fullmatch(value) or value in {".", ".."}:
        raise Refused(f"{where}: expected a safe basename")
    return value


def _utc(s: Any, where: str) -> int:
    if not isinstance(s, str) or not re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", s):
        raise Refused(f"{where}: expected whole-second UTC ISO timestamp")
    import datetime

    try:
        t = datetime.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc)
    except ValueError as exc:
        raise Refused(f"{where}: invalid UTC timestamp") from exc
    return int(t.timestamp())


def _validate_manifest(path: Path) -> tuple[dict[str, Any], str]:
    raw = _read_guarded(path, LIMITS["max_manifest_bytes"])
    try:
        text = raw.decode("utf-8", "strict")
        doc = json.loads(text, object_pairs_hook=_object_no_duplicates, parse_constant=_reject_constant)
    except RecursionError as exc:
        raise Refused("manifest exceeds the supported JSON nesting depth") from exc
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise Refused(f"manifest is not strict UTF-8 JSON: {exc}") from exc
    top = _exact_keys(doc, {"schema", "day", "driver_sha256", "release", "clip", "window", "candidate", "replication", "replication_scorer", "replication_scorer_relation", "builds", "public_arms", "compare_arms", "limits"}, "manifest")
    if top["schema"] != SCHEMA or top["day"] != DAY:
        raise Refused(f"manifest schema/day must be {SCHEMA} / {DAY}")
    if not isinstance(top["driver_sha256"], str) or not HEX64.fullmatch(top["driver_sha256"]):
        raise Refused("manifest.driver_sha256 must be a lowercase SHA-256")
    driver_bytes = _read_guarded(SCRIPT_PATH, 2 * 1024 * 1024)
    if _sha(driver_bytes) != top["driver_sha256"]:
        raise Refused("manifest driver SHA-256 does not match this wrapper source")

    rel = _exact_keys(top["release"], {"tag", "inventory_url", "inventory_sha256", "parts"}, "release")
    if not isinstance(rel["tag"], str) or not re.fullmatch(r"v?2026[._-]11[._-]30[-A-Za-z0-9._-]*", rel["tag"]):
        raise Refused("release.tag must identify the complete Nov 30 2026 archive release")
    if rel["inventory_url"] != f"https://github.com/adsblol/globe_history_2026/releases/tag/{rel['tag']}":
        raise Refused("release.inventory_url must be the canonical adsb.lol release page")
    if not isinstance(rel["inventory_sha256"], str) or not HEX64.fullmatch(rel["inventory_sha256"]):
        raise Refused("release.inventory_sha256 must be lowercase SHA-256")
    parts = rel["parts"]
    if not isinstance(parts, list) or not 1 <= len(parts) <= LIMITS["max_archive_parts"]:
        raise Refused("release.parts must contain 1..16 entries")
    seen: set[str] = set()
    total = 0
    for i, item in enumerate(parts):
        p = _exact_keys(item, {"name", "bytes", "sha256"}, f"release.parts[{i}]")
        name = _safe_basename(p["name"], f"release.parts[{i}].name")
        if name in seen:
            raise Refused(f"duplicate archive part {name}")
        seen.add(name)
        n = p["bytes"]
        if type(n) is not int or not 0 < n <= LIMITS["max_part_bytes"]:
            raise Refused(f"{name}: size must be 1..8 GiB")
        if not isinstance(p["sha256"], str) or not HEX64.fullmatch(p["sha256"]):
            raise Refused(f"{name}: malformed SHA-256")
        total += n
        if total > LIMITS["max_archive_bytes"]:
            raise Refused("archive inventory exceeds 64 GiB")
    if [p["name"] for p in parts] != sorted(seen, key=os.fsencode):
        raise Refused("archive inventory must be in canonical bytewise filename order")
    inventory = {"tag": rel["tag"], "inventory_url": rel["inventory_url"], "parts": parts}
    got_inventory = _sha(json.dumps(inventory, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode("ascii"))
    if got_inventory != rel["inventory_sha256"]:
        raise Refused("release inventory digest does not match its canonical part list")

    clip = _exact_keys(top["clip"], {"box", "frame_ms"}, "clip")
    if clip["box"] != ZAU_BOX or type(clip["frame_ms"]) is not int or clip["frame_ms"] != 1000:
        raise Refused("P-010 v1 clip profile must use the fixed ZAU box and 1000 ms frame")

    win = _exact_keys(top["window"], {"archive_from", "archive_to", "warmup_from", "score_from", "score_to"}, "window")
    expected = {"archive_from": ARCHIVE_FROM, "archive_to": ARCHIVE_TO, "warmup_from": WARMUP_FROM, "score_from": SCORE_FROM, "score_to": SCORE_TO}
    if win != expected:
        raise Refused("P-010 v1 date/window bounds do not match the frozen full-day, warm-up and score intervals")
    if not (_utc(win["archive_from"], "archive_from") < _utc(win["warmup_from"], "warmup_from") < _utc(win["score_from"], "score_from") < _utc(win["score_to"], "score_to") == _utc(win["archive_to"], "archive_to")):
        raise Refused("P-010 window ordering is invalid")

    for role in ("candidate", "replication"):
        src = _exact_keys(top[role], {"tree", "commit"}, role)
        if not isinstance(src["tree"], str) or not Path(src["tree"]).is_absolute():
            raise Refused(f"{role}.tree must be an absolute path")
        if not isinstance(src["commit"], str) or not HEX40.fullmatch(src["commit"]):
            raise Refused(f"{role}.commit must be 40 lowercase hex digits")
    if top["replication"]["commit"] != REPLICATION_TRACKER_COMMIT:
        raise Refused("replication tracker source must be the exact frozen C commit")

    patched = _exact_keys(top["replication_scorer"], {"tree", "commit"}, "replication_scorer")
    if not isinstance(patched["tree"], str) or not Path(patched["tree"]).is_absolute():
        raise Refused("replication_scorer.tree must be an absolute path")
    if not isinstance(patched["commit"], str) or not HEX40.fullmatch(patched["commit"]):
        raise Refused("replication_scorer.commit must be 40 lowercase hex digits")
    relation = _exact_keys(top["replication_scorer_relation"], {"base_commit", "scorer_commit", "changed_paths", "delta_sha256", "review_artifact", "review_sha256"}, "replication_scorer_relation")
    if relation["base_commit"] != top["replication"]["commit"] or relation["scorer_commit"] != patched["commit"]:
        raise Refused("replication scorer patch must be based on and bound to the exact C tracker source")
    paths = relation["changed_paths"]
    if not isinstance(paths, list) or paths != SCORER_PATCH_PATHS:
        raise Refused("replication scorer derivative must contain only the registered P-002/ERAM-score support paths")
    for i, p in enumerate(paths):
        if not isinstance(p, str) or p.startswith("/") or ".." in Path(p).parts or "\x00" in p:
            raise Refused(f"replication_scorer_relation.changed_paths[{i}] is unsafe")
    for key in ("delta_sha256", "review_sha256"):
        value = relation[key]
        if not isinstance(value, str) or not HEX64.fullmatch(value):
            raise Refused(f"replication_scorer_relation.{key} must be lowercase SHA-256")
    if not isinstance(relation["review_artifact"], str) or not Path(relation["review_artifact"]).is_absolute():
        raise Refused("replication_scorer_relation.review_artifact must be an absolute path to the frozen source review")

    builds = _exact_keys(top["builds"], set(BUILD_ROLES), "builds")
    build_commits = {
        "clipper": top["candidate"]["commit"],
        "candidate": top["candidate"]["commit"],
        "candidate_scorer": top["candidate"]["commit"],
        "replication_scorer": patched["commit"],
    }
    target_dirs: dict[str, Path] = {}
    for role, expected_commit in build_commits.items():
        b = _exact_keys(builds[role], {"metadata", "metadata_sha256", "binary", "binary_sha256", "artifact_name", "target_dir"}, f"builds.{role}")
        if not isinstance(b["metadata"], str) or not Path(b["metadata"]).is_absolute():
            raise Refused(f"builds.{role}.metadata must be an absolute path")
        if not isinstance(b["binary"], str) or not Path(b["binary"]).is_absolute():
            raise Refused(f"builds.{role}.binary must be an absolute path")
        for key in ("metadata", "binary"):
            artifact_path = Path(b[key])
            _guard().check(artifact_path)
            if artifact_path.resolve(strict=False) != artifact_path:
                raise Refused(f"builds.{role}.{key} must be an absolute canonical path")
        if not isinstance(b["metadata_sha256"], str) or not HEX64.fullmatch(b["metadata_sha256"]):
            raise Refused(f"builds.{role}.metadata_sha256 is malformed")
        if not isinstance(b["binary_sha256"], str) or not HEX64.fullmatch(b["binary_sha256"]):
            raise Refused(f"builds.{role}.binary_sha256 is malformed")
        if b["artifact_name"] != ARTIFACT_NAMES[role]:
            raise Refused(f"builds.{role}.artifact_name must be {ARTIFACT_NAMES[role]}")
        target_dirs[role] = _canonical_build_path(b["target_dir"], f"builds.{role}.target_dir")

    # A distinct fresh target per role prevents candidate/C and tool/scorer artifacts from
    # silently sharing Cargo's mutable target cache. Parent/child overlaps are also forbidden.
    for i, role in enumerate(BUILD_ROLES):
        for other in BUILD_ROLES[i + 1:]:
            left, right = target_dirs[role], target_dirs[other]
            if left == right or _path_is_within(left, right) or _path_is_within(right, left):
                raise Refused(f"build target directories overlap: {role} and {other}")
    source_roots = []
    for key in ("candidate", "replication", "replication_scorer"):
        source_path = Path(top[key]["tree"])
        _guard().check(source_path)
        source_roots.append(source_path.resolve(strict=False))
    for role, target in target_dirs.items():
        if any(_path_is_within(target, src) or _path_is_within(src, target) for src in source_roots):
            raise Refused(f"build target directory for {role} overlaps a source checkout")

    pub = top["public_arms"]
    comp = top["compare_arms"]
    if not isinstance(pub, list) or not 1 <= len(pub) <= 2 or not isinstance(comp, list) or len(comp) > 4:
        raise Refused("public_arms needs 1..2 arms and compare_arms needs at most 4 arms")
    for i, item in enumerate(pub):
        arm = _exact_keys(item, {"name", "timing_profile"}, f"public_arms[{i}]")
        if not isinstance(arm["name"], str) or not ARM_NAME.fullmatch(arm["name"]):
            raise Refused(f"public_arms[{i}].name is invalid")
        if arm["timing_profile"] not in TIMING_PROFILES:
            raise Refused(f"public_arms[{i}].timing_profile is not a declared P-010 profile")
    if len({a["name"] for a in pub}) != len(pub) or len({a["timing_profile"] for a in pub}) != len(pub):
        raise Refused("public arm names and timing profiles must be unique")
    for i, item in enumerate(comp):
        arm = _exact_keys(item, {"name", "tracker_role", "timing_profile", "publication_grid"}, f"compare_arms[{i}]")
        if not isinstance(arm["name"], str) or not ARM_NAME.fullmatch(arm["name"]):
            raise Refused(f"compare_arms[{i}].name is invalid")
        if arm["tracker_role"] not in {"candidate", "replication"}:
            raise Refused(f"compare_arms[{i}].tracker_role must identify candidate or frozen C replication")
        if arm["timing_profile"] not in TIMING_PROFILES or arm["publication_grid"] not in GRID_PROFILES:
            raise Refused(f"compare_arms[{i}] has an unsupported timing/grid profile")
        expected_profile = "fixed-10-30-120-v1" if arm["tracker_role"] == "candidate" else "legacy-day-intervals"
        if arm["timing_profile"] != expected_profile:
            raise Refused(f"compare_arms[{i}] timing profile does not match its declared tracker-source role")
    if len({a["name"] for a in comp}) != len(comp):
        raise Refused("compare arm names must be unique")
    _validate_compare_pairs(comp)

    limits = _exact_keys(top["limits"], set(LIMITS), "limits")
    if limits != LIMITS:
        raise Refused("manifest limits must equal the frozen P-010 v1 ceilings")
    return doc, _sha(raw)


def _capture_source_command(argv: list[str], limit: int) -> bytes:
    # C7: cap pipes while draining; communicate()/run(PIPE) would allocate before checking.
    if not 0 < limit <= 4 * 1024 * 1024:
        raise Refused("invalid source-command stdout cap")
    proc = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, start_new_session=True, close_fds=True, env=CHILD_ENV)
    selector = selectors.DefaultSelector()
    outputs = {"stdout": bytearray(), "stderr": bytearray()}
    try:
        for pipe, name in ((proc.stdout, "stdout"), (proc.stderr, "stderr")):
            if pipe is None:
                raise Refused("source-command pipe missing")
            os.set_blocking(pipe.fileno(), False)
            selector.register(pipe, selectors.EVENT_READ, name)
        deadline = time.monotonic() + 30
        while selector.get_map() or proc.poll() is None:
            if time.monotonic() >= deadline:
                raise Refused("source command exceeded 30 seconds")
            for key, _ in selector.select(0.1):
                block = os.read(key.fileobj.fileno(), 65536)
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                output = outputs[key.data]
                cap = limit if key.data == "stdout" else 64 * 1024
                if len(output) + len(block) > cap:
                    raise Refused("source command exceeded output cap")
                output.extend(block)
        if proc.wait() != 0:
            raise Refused("source command failed: " + outputs["stderr"][:512].decode("utf-8", "replace"))
        return bytes(outputs["stdout"])
    finally:
        _stop_owned(proc)
        selector.close()
        for pipe in (proc.stdout, proc.stderr):
            if pipe is not None and not pipe.closed:
                pipe.close()


def _git(tree: Path, *args: str, limit: int = 1024 * 1024) -> bytes:
    try:
        return _capture_source_command(["git", "-C", str(tree), *args], limit)
    except OSError as exc:
        raise Refused(f"cannot inspect source tree {tree}: {exc}") from exc


def _source_state(tree_text: str, expected_commit: str) -> dict[str, str]:
    tree = Path(tree_text)
    _guard().check(tree)
    if not tree.is_dir() or tree.is_symlink():
        raise Refused(f"source tree is not a plain directory: {tree}")
    head = _git(tree, "rev-parse", "HEAD").decode("ascii", "strict").strip()
    status = _git(tree, "status", "--porcelain=v1", "--untracked-files=all", limit=2 * 1024 * 1024)
    if head != expected_commit or status:
        raise Refused(f"source tree must be clean at pinned commit {expected_commit}")
    return {"commit": head, "status": ""}


def _source_relation(doc: dict[str, Any]) -> None:
    candidate = Path(doc["candidate"]["tree"])
    base = Path(doc["replication"]["tree"])
    scorer = Path(doc["replication_scorer"]["tree"])
    rel = doc["replication_scorer_relation"]
    common_candidate = Path(os.fsdecode(_git(candidate, "rev-parse", "--git-common-dir").strip()))
    common_base = Path(os.fsdecode(_git(base, "rev-parse", "--git-common-dir").strip()))
    common_scorer = Path(os.fsdecode(_git(scorer, "rev-parse", "--git-common-dir").strip()))
    if not common_candidate.is_absolute():
        common_candidate = (candidate / common_candidate).resolve()
    if not common_base.is_absolute():
        common_base = (base / common_base).resolve()
    if not common_scorer.is_absolute():
        common_scorer = (scorer / common_scorer).resolve()
    if common_candidate.resolve() != common_base.resolve() or common_base.resolve() != common_scorer.resolve():
        raise Refused("candidate, frozen C tracker and C scorer derivative must share one Git object database")
    c_commit, scorer_commit = rel["base_commit"], rel["scorer_commit"]
    _git(candidate, "merge-base", "--is-ancestor", c_commit, doc["candidate"]["commit"], limit=4096)
    if _git(base, "merge-base", "--is-ancestor", c_commit, scorer_commit, limit=4096) != b"":
        raise Refused("C scorer derivative does not descend from the exact frozen C tracker commit")
    changed = _git(base, "diff", "--name-only", "--no-renames", c_commit, scorer_commit, "--", limit=1024 * 1024).decode("utf-8", "strict").splitlines()
    if changed != rel["changed_paths"] or changed != SCORER_PATCH_PATHS:
        raise Refused("C scorer derivative changed files outside the reviewed P-002/ERAM-score support paths")
    diff = _git(base, "diff", "--binary", "--no-ext-diff", "--no-renames", c_commit, scorer_commit, "--", limit=8 * 1024 * 1024)
    if _sha(diff) != rel["delta_sha256"]:
        raise Refused("C scorer-only source patch hash differs from the frozen manifest")
    review_raw = _read_guarded(Path(rel["review_artifact"]), 256 * 1024)
    if _sha(review_raw) != rel["review_sha256"]:
        raise Refused("C scorer-only review artifact hash differs from the frozen manifest")


def _build_source(doc: dict[str, Any], role: str) -> dict[str, str]:
    if role in {"clipper", "candidate", "candidate_scorer"}:
        return doc["candidate"]
    if role == "replication_scorer":
        return doc["replication_scorer"]
    raise Refused(f"unknown build source role {role}")


def _require_target_file(raw: Any, target_dir: Path, where: str) -> Path:
    if not isinstance(raw, str) or not Path(raw).is_absolute():
        raise Refused(f"{where} must be an absolute path")
    path = Path(raw)
    _guard().check(path)
    _guard().check(target_dir)
    resolved = path.resolve(strict=True)
    if resolved != path:
        raise Refused(f"{where} must use its canonical non-symlink path")
    target = target_dir.resolve(strict=True)
    if not _path_is_within(resolved, target) or resolved == target:
        raise Refused(f"{where} must resolve below its exclusive target directory")
    return resolved


def _run_guard_probe(binary: Path, expected: bytes) -> None:
    proc = subprocess.Popen([str(binary)], cwd=ROOT, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            start_new_session=True, close_fds=True, env=CHILD_ENV)
    selector = selectors.DefaultSelector()
    outputs: dict[int, bytearray] = {}
    stdout_fd = proc.stdout.fileno() if proc.stdout is not None else -1
    stderr_fd = proc.stderr.fileno() if proc.stderr is not None else -1
    timed_out = False
    overflow = False
    started = time.monotonic()
    try:
        for pipe in (proc.stdout, proc.stderr):
            if pipe is not None:
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ)
                outputs[pipe.fileno()] = bytearray()
        while proc.poll() is None or selector.get_map():
            if time.monotonic() - started >= 5.0:
                timed_out = True
                break
            for key, _ in selector.select(0.1):
                block = os.read(key.fd, 512)
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                buf = outputs[key.fd]
                if len(buf) + len(block) > MAX_PROBE_BYTES:
                    overflow = True
                    break
                buf.extend(block)
            if overflow:
                break
        if timed_out or overflow:
            _stop_owned(proc)
        else:
            proc.wait()
    finally:
        selector.close()
        if proc.poll() is None:
            _stop_owned(proc)
        for pipe in (proc.stdout, proc.stderr):
            if pipe is not None and not pipe.closed:
                pipe.close()
    stdout = bytes(outputs.get(stdout_fd, b""))
    stderr = bytes(outputs.get(stderr_fd, b""))
    if timed_out or overflow or proc.returncode != 0 or stdout != expected or stderr:
        raise Refused("compiled oa-sealed guard probe did not match its exact synthetic rehearsal receipt")


def _check_build(doc: dict[str, Any], role: str) -> Path:
    cargo_executable, rustc_executable, _, build_path = rust_toolchain()
    b = doc["builds"][role]
    meta_path = Path(b["metadata"])
    binary = Path(b["binary"])
    target_dir = _canonical_build_path(b["target_dir"], f"builds.{role}.target_dir")
    _guard().check(meta_path)
    _guard().check(binary)
    meta_raw = _read_guarded(meta_path, MAX_BUILD_RECEIPT_BYTES)
    if _sha(meta_raw) != b["metadata_sha256"]:
        raise Refused(f"{role} build metadata SHA-256 differs from manifest")
    try:
        meta = json.loads(meta_raw.decode("utf-8"), object_pairs_hook=_object_no_duplicates, parse_constant=_reject_constant)
    except RecursionError as exc:
        raise Refused("build receipt exceeds the supported JSON nesting depth") from exc
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise Refused(f"{role} build metadata is invalid: {exc}") from exc
    required = {"schema", "role", "toolchain", "toolchain_trust_assumption", "toolchain_identity", "source_before", "source_after", "argv", "exit_code", "timed_out", "capture_limit_exceeded", "non_test", "target_dir", "target_dir_was_absent_before_build", "cargo_target_dir", "cargo_wrapper_environment", "artifacts", "local_artifacts", "target_usage", "guard_binding"}
    _exact_keys(meta, required, f"{role} build metadata")
    if meta["schema"] != BUILD_RECEIPT_SCHEMA or meta["role"] != role or meta["toolchain"] != "1.89.0":
        raise Refused(f"{role} build metadata schema/role/toolchain mismatch")
    if meta["toolchain_trust_assumption"] != TOOLCHAIN_TRUST_ASSUMPTION:
        raise Refused(f"{role} receipt overstates the trusted-host compiler evidence")
    if (meta["target_dir"] != str(target_dir) or meta["cargo_target_dir"] != str(target_dir)
            or meta["target_dir_was_absent_before_build"] is not True
            or meta["cargo_wrapper_environment"] != {
                "RUSTC_WRAPPER": None, "RUSTC_WORKSPACE_WRAPPER": None,
                "RUSTFLAGS": None, "CARGO_BUILD_RUSTFLAGS": None,
                "CARGO_ENCODED_RUSTFLAGS": "", "CARGO_BUILD_RUSTC_WRAPPER": None,
                "CARGO_TARGET_DIR_inherited": False, "PATH": build_path,
                "RUSTC": rustc_executable,
            }):
        raise Refused(f"{role} build receipt does not prove its isolated fresh target environment")
    toolchain_identity = _exact_keys(meta["toolchain_identity"], {"before", "after"}, f"{role}.toolchain_identity")
    toolchain_before = _exact_keys(toolchain_identity["before"], {"cargo", "rustc"}, f"{role}.toolchain_identity.before")
    toolchain_after = _exact_keys(toolchain_identity["after"], {"cargo", "rustc"}, f"{role}.toolchain_identity.after")
    if toolchain_before != toolchain_after:
        raise Refused(f"{role} trusted Rust toolchain changed during its build/probe")
    for tool_name, expected_path, expected_prefix in (
            ("cargo", cargo_executable, "cargo 1.89.0 "),
            ("rustc", rustc_executable, "rustc 1.89.0 ")):
        tool = _exact_keys(toolchain_before.get(tool_name), {"path", "bytes", "sha256", "version"},
                           f"{role}.toolchain_identity.{tool_name}")
        if (tool["path"] != expected_path or type(tool["bytes"]) is not int or not 1 <= tool["bytes"] <= 128 * 1024 * 1024
                or not isinstance(tool["sha256"], str) or not HEX64.fullmatch(tool["sha256"])
                or not isinstance(tool["version"], str) or not tool["version"].startswith(expected_prefix)):
            raise Refused(f"{role} receipt does not identify the trusted installed {tool_name} 1.89 tool")
        size, digest = _hash_file(Path(expected_path), 128 * 1024 * 1024)
        if size != tool["bytes"] or digest != tool["sha256"]:
            raise Refused(f"{role} trusted {tool_name} executable changed since its receipt")
    source_obj = _build_source(doc, role)
    commit = source_obj["commit"]
    before = _exact_keys(meta["source_before"], {"commit", "status"}, f"{role}.source_before")
    after = _exact_keys(meta["source_after"], {"commit", "status"}, f"{role}.source_after")
    if before != {"commit": commit, "status": ""} or after != before:
        raise Refused(f"{role} build was not completed from the exact pinned clean source tree")
    _source_state(source_obj["tree"], commit)
    if (meta["argv"] != _build_argv(role, target_dir) or type(meta["exit_code"]) is not int or meta["exit_code"] != 0
            or meta["timed_out"] is not False or meta["capture_limit_exceeded"] is not False or meta["non_test"] is not True):
        raise Refused(f"{role} build metadata does not prove the pinned non-test release build")
    artifacts = meta["artifacts"]
    if not isinstance(artifacts, dict) or b["artifact_name"] not in artifacts:
        raise Refused(f"{role} build metadata lacks artifact {b['artifact_name']}")
    a = artifacts[b["artifact_name"]]
    if not isinstance(a, dict) or set(a) != {"binary", "sha256", "cargo_artifact"}:
        raise Refused(f"{role} build artifact metadata is incomplete")
    artifact_binary = _require_target_file(a["binary"], target_dir, f"{role}.artifact.binary")
    if artifact_binary != binary.resolve(strict=True):
        raise Refused(f"{role} build artifact path differs from manifest")
    if a["sha256"] != b["binary_sha256"]:
        raise Refused(f"{role} build artifact hash differs from manifest")
    ca = a["cargo_artifact"]
    if not isinstance(ca, dict) or set(ca) != {"manifest_path", "target_name", "fresh", "filenames"}:
        raise Refused(f"{role} Cargo artifact does not prove a fresh compiler output")
    package_path = {"clipper": "oa-adsb", "candidate": "oa-track-tools",
                    "candidate_scorer": "oa-track-tools", "replication_scorer": "oa-track"}[role]
    expected_target_manifest = Path(source_obj["tree"]) / "crates" / package_path / "Cargo.toml"
    if (ca["manifest_path"] != str(expected_target_manifest) or ca["target_name"] != b["artifact_name"]
            or ca["fresh"] is not False or not isinstance(ca["filenames"], list)):
        raise Refused(f"{role} Cargo artifact does not prove the exact fresh target")
    filenames = [_require_target_file(item, target_dir, f"{role}.cargo_artifact.filenames") for item in ca["filenames"]]
    if artifact_binary not in filenames:
        raise Refused(f"{role} executable is not one of the fresh Cargo artifact filenames")

    local_artifacts = meta["local_artifacts"]
    if not isinstance(local_artifacts, list) or not 1 <= len(local_artifacts) <= MAX_LOCAL_ARTIFACTS:
        raise Refused(f"{role} local Cargo artifact inventory is absent or exceeds its cap")
    local_packages: dict[str, tuple[Path, str]] = {}
    artifact_keys: set[tuple[str, str, tuple[str, ...]]] = set()
    local_files: set[Path] = set()
    total_local_bytes = 0
    _guard().check(Path(source_obj["tree"]))
    source_root = Path(source_obj["tree"]).resolve(strict=True)
    for index, item in enumerate(local_artifacts):
        local = _exact_keys(item, {"package_id", "manifest_path", "package_tree_git_oid", "target_name", "kind", "fresh", "filenames"}, f"{role}.local_artifacts[{index}]")
        if (not isinstance(local["package_id"], str) or not local["package_id"] or len(local["package_id"]) > 4096
                or not isinstance(local["manifest_path"], str) or not Path(local["manifest_path"]).is_absolute()
                or not isinstance(local["package_tree_git_oid"], str) or not HEX40.fullmatch(local["package_tree_git_oid"])
                or not isinstance(local["target_name"], str) or not local["target_name"]
                or not isinstance(local["kind"], list) or not 1 <= len(local["kind"]) <= 8
                or any(not isinstance(kind, str) or not kind for kind in local["kind"])
                or len(set(local["kind"])) != len(local["kind"])
                or local["fresh"] is not False or not isinstance(local["filenames"], list)
                or not 1 <= len(local["filenames"]) <= 128):
            raise Refused(f"{role} local Cargo artifact has malformed or reused metadata")
        manifest = Path(local["manifest_path"])
        _guard().check(manifest)
        if manifest.resolve(strict=True) != manifest or manifest.name != "Cargo.toml" or not _path_is_within(manifest, source_root):
            raise Refused(f"{role} local Cargo artifact names a noncanonical or foreign local package")
        package_tree_path = manifest.parent.relative_to(source_root)
        git_path = package_tree_path.as_posix() or "."
        actual_tree = _git(source_root, "rev-parse", f"{source_obj['commit']}:{git_path}", limit=4096).decode("ascii", "strict").strip()
        if actual_tree != local["package_tree_git_oid"]:
            raise Refused(f"{role} local Cargo artifact package tree differs from its source commit")
        prior = local_packages.get(local["package_id"])
        if prior is not None and prior != (manifest, actual_tree):
            raise Refused(f"{role} package ID maps to conflicting local manifest identities")
        local_packages[local["package_id"]] = (manifest, actual_tree)
        identity = (local["package_id"], local["target_name"], tuple(local["kind"]))
        if identity in artifact_keys:
            raise Refused(f"{role} local Cargo artifact metadata contains a duplicate target")
        artifact_keys.add(identity)
        for file_index, file_item in enumerate(local["filenames"]):
            file_record = _exact_keys(file_item, {"path", "bytes", "sha256"}, f"{role}.local_artifacts[{index}].filenames[{file_index}]")
            artifact_path = _require_target_file(file_record["path"], target_dir, f"{role}.local_artifacts[{index}].filenames[{file_index}].path")
            if artifact_path in local_files or type(file_record["bytes"]) is not int or not 0 <= file_record["bytes"] <= LIMITS["max_output_file_bytes"] or not isinstance(file_record["sha256"], str) or not HEX64.fullmatch(file_record["sha256"]):
                raise Refused(f"{role} local Cargo artifact file metadata is duplicate or malformed")
            size, file_sha = _hash_file(artifact_path, LIMITS["max_output_file_bytes"])
            if size != file_record["bytes"] or file_sha != file_record["sha256"]:
                raise Refused(f"{role} local Cargo artifact file differs from its receipt")
            total_local_bytes += size
            if total_local_bytes > MAX_LOCAL_ARTIFACT_BYTES:
                raise Refused(f"{role} local Cargo artifact inventory exceeds its byte cap")
            local_files.add(artifact_path)
    if artifact_binary not in local_files:
        raise Refused(f"{role} selected executable is missing from the local artifact inventory")
    usage = _exact_keys(meta["target_usage"], {"files", "directories", "bytes", "max_files", "max_directories", "max_bytes", "max_depth"}, f"{role}.target_usage")
    if (any(type(usage[k]) is not int for k in ("files", "directories", "bytes"))
            or usage["max_files"] != MAX_BUILD_TARGET_FILES
            or usage["max_directories"] != MAX_BUILD_TARGET_DIRECTORIES
            or usage["max_bytes"] != MAX_BUILD_TARGET_BYTES
            or usage["max_depth"] != MAX_BUILD_TARGET_DEPTH
            or not 1 <= usage["files"] <= MAX_BUILD_TARGET_FILES
            or not 1 <= usage["directories"] <= MAX_BUILD_TARGET_DIRECTORIES
            or not 1 <= usage["bytes"] <= MAX_BUILD_TARGET_BYTES):
        raise Refused(f"{role} target directory usage is malformed or exceeds its fixed cap")

    guard = _exact_keys(meta["guard_binding"], {"source_list", "source_list_sha256", "sealed_rlib", "sealed_rlib_sha256", "digest_rlib", "digest_rlib_sha256", "oa_sealed_cargo_artifact", "oa_sha256_cargo_artifact", "depinfo", "depinfo_sha256", "probe_source", "probe_source_sha256", "probe_binary", "probe_binary_sha256", "probe_rustc_argv", "probe_exit_code", "probe_stdout"}, f"{role}.guard_binding")
    source_obj = _build_source(doc, role)
    list_path = Path(source_obj["tree"]) / "crates/oa-sealed/sealed_paths.txt"
    _guard().check(list_path)
    if not isinstance(guard["source_list"], str) or not Path(guard["source_list"]).is_absolute():
        raise Refused(f"{role} seal-list source path must be absolute")
    _guard().check(Path(guard["source_list"]))
    if Path(guard["source_list"]) != list_path.resolve(strict=True):
        raise Refused(f"{role} compiled seal-list source path differs from the pinned checkout")
    _, list_sha = _hash_file(list_path, 2 * 1024 * 1024)
    if not isinstance(guard["source_list_sha256"], str) or not HEX64.fullmatch(guard["source_list_sha256"]) or guard["source_list_sha256"] != list_sha:
        raise Refused(f"{role} compiled seal-list source digest differs from its source checkout")
    sealed_rlib_path: Path | None = None
    for library, path_field, hash_field in (("oa_sealed", "sealed_rlib", "sealed_rlib_sha256"),
                                             ("oa_sha256", "digest_rlib", "digest_rlib_sha256")):
        event = _exact_keys(guard[f"{library}_cargo_artifact"], {"manifest_path", "fresh", "filenames"}, f"{role}.{library}_cargo_artifact")
        expected_manifest = Path(source_obj["tree"]) / "crates" / ("oa-sealed" if library == "oa_sealed" else "oa-sha256") / "Cargo.toml"
        if event["manifest_path"] != str(expected_manifest) or event["fresh"] is not False or not isinstance(event["filenames"], list):
            raise Refused(f"{role} Cargo artifact does not prove a fresh {library} library build")
        artifact_paths = [_require_target_file(item, target_dir, f"{role}.{library}.filenames") for item in event["filenames"]]
        rlib = _require_target_file(guard[path_field], target_dir, f"{role}.{path_field}")
        if not str(rlib).endswith(".rlib") or rlib not in artifact_paths:
            raise Refused(f"{role} probe input is not the exact Cargo {library} rlib")
        if library == "oa_sealed":
            sealed_rlib_path = rlib
        _, actual_sha = _hash_file(rlib, LIMITS["max_output_file_bytes"])
        if guard[hash_field] != actual_sha:
            raise Refused(f"{role} compiled {library} rlib hash differs from its receipt")
    depinfo = _require_target_file(guard["depinfo"], target_dir, f"{role}.depinfo")
    if sealed_rlib_path is None:
        raise Refused(f"{role} has no exact oa-sealed rlib for dep-info binding")
    expected_depinfo = target_dir / "release" / "deps" / (sealed_rlib_path.name.removeprefix("lib").removesuffix(".rlib") + ".d")
    if depinfo != expected_depinfo.resolve(strict=True):
        raise Refused(f"{role} dep-info path does not correspond to the selected oa-sealed rlib")
    dep_bytes = _read_guarded(depinfo, 2 * 1024 * 1024)
    if _sha(dep_bytes) != guard["depinfo_sha256"]:
        raise Refused(f"{role} oa-sealed dep-info hash differs from its receipt")
    dep_text = dep_bytes.decode("utf-8", "strict")
    if "crates/oa-sealed/src/lib.rs" not in dep_text or "crates/oa-sealed/src/../sealed_paths.txt" not in dep_text:
        raise Refused(f"{role} dep-info does not bind the source list included by oa-sealed")
    if not isinstance(guard["probe_source_sha256"], str) or guard["probe_source_sha256"] != GUARD_PROBE_SHA256:
        raise Refused(f"{role} guard probe source differs from the fixed wrapper probe")
    probe_source = _require_target_file(guard["probe_source"], target_dir, f"{role}.probe_source")
    if _read_guarded(probe_source, 16 * 1024).decode("utf-8", "strict") != GUARD_PROBE_SOURCE:
        raise Refused(f"{role} probe file differs from the fixed synthetic source")
    probe_binary = _require_target_file(guard["probe_binary"], target_dir, f"{role}.probe_binary")
    probe_st = probe_binary.lstat()
    if not stat.S_ISREG(probe_st.st_mode) or not os.access(probe_binary, os.X_OK):
        raise Refused(f"{role} compiled guard probe must be a regular executable")
    _, probe_sha = _hash_file(probe_binary, LIMITS["max_output_file_bytes"])
    if guard["probe_binary_sha256"] != probe_sha:
        raise Refused(f"{role} compiled guard probe hash differs from its receipt")
    expected_rustc = [rustc_executable, "--edition=2021", str(probe_source), "-L",
                      f"dependency={target_dir}/release/deps", "--extern", f"oa_sealed={guard['sealed_rlib']}",
                      "--extern", f"oa_sha256={guard['digest_rlib']}", "-o", str(probe_binary)]
    if guard["probe_rustc_argv"] != expected_rustc or type(guard["probe_exit_code"]) is not int or guard["probe_exit_code"] != 0:
        raise Refused(f"{role} guard probe was not compiled against the exact recorded Cargo rlibs")
    expected_probe = f"P010_GUARD_PROBE_OK v1 list_sha256={list_sha} paths=4 timestamps=4 opens=1\n".encode("ascii")
    if guard["probe_stdout"] != expected_probe.decode("ascii"):
        raise Refused(f"{role} guard probe receipt does not match the exact source-list digest")
    _run_guard_probe(probe_binary, expected_probe)
    st = binary.lstat()
    if not stat.S_ISREG(st.st_mode) or not os.access(binary, os.X_OK):
        raise Refused(f"{role} binary must be a regular executable")
    _, digest = _hash_file(binary, LIMITS["max_output_file_bytes"])
    if digest != b["binary_sha256"]:
        raise Refused(f"{role} binary SHA-256 differs from manifest")
    return binary


def _preflight(doc: dict[str, Any], role: str = "all") -> dict[str, Path]:
    source_roles = ("candidate",) if role == "public" else ("candidate", "replication", "replication_scorer")
    for k in source_roles:
        _source_state(doc[k]["tree"], doc[k]["commit"])
    if role != "public":
        _source_relation(doc)
    roles = ("clipper", "candidate", "candidate_scorer", "replication_scorer") if role in {"all", "compare"} else ("clipper", "candidate")
    return {r: _check_build(doc, r) for r in roles}


def _verify_inventory(archive_dir: Path, doc: dict[str, Any]) -> list[Path]:
    _guard().check(archive_dir)
    if archive_dir.is_symlink() or not archive_dir.is_dir():
        raise Refused("archive directory must be a plain directory")
    parts = doc["release"]["parts"]
    wanted = [p["name"] for p in parts]
    actual: list[str] = []
    with os.scandir(archive_dir) as entries:
        for entry in entries:
            actual.append(entry.name)
            if len(actual) > LIMITS["max_archive_parts"]:
                raise Refused("archive directory has more files than the bounded inventory")
    actual.sort(key=os.fsencode)
    if actual != wanted:
        missing = sorted(set(wanted) - set(actual), key=os.fsencode)
        extra = sorted(set(actual) - set(wanted), key=os.fsencode)
        raise Refused(f"archive directory does not equal the complete manifest inventory (missing={missing[:3]}, extra={extra[:3]})")
    paths: list[Path] = []
    total = 0
    for p in parts:
        path = archive_dir / p["name"]
        if path.is_symlink():
            raise Refused(f"archive part is a symlink: {p['name']}")
        size, digest = _hash_file(path, LIMITS["max_part_bytes"])
        if size != p["bytes"] or digest != p["sha256"]:
            raise Refused(f"archive part size/SHA-256 mismatch: {p['name']}")
        total += size
        if total > LIMITS["max_archive_bytes"]:
            raise Refused("archive bytes exceed 64 GiB")
        paths.append(path.resolve(strict=True))
    return paths


def _measure_dir(root: Path, max_files: int, max_bytes: int) -> tuple[int, int]:
    _guard().check(root)
    if root.is_symlink() or not root.is_dir():
        raise Refused("raw SFDPS input must be a plain directory")
    nfiles = 0
    nbytes = 0
    ndirectories = 1
    stack: list[tuple[Path, int]] = [(root, 0)]
    visited: set[tuple[int, int]] = set()
    while stack:
        directory, depth = stack.pop()
        if depth > MAX_DEPTH:
            raise Refused("raw SFDPS input exceeds directory-depth bound")
        st_dir = directory.stat(follow_symlinks=False)
        ident = (st_dir.st_dev, st_dir.st_ino)
        if ident in visited:
            raise Refused("raw SFDPS input contains a repeated directory inode")
        visited.add(ident)
        with os.scandir(directory) as it:
            for entry in it:
                path = Path(entry.path)
                _guard().check(path)
                st = entry.stat(follow_symlinks=False)
                if stat.S_ISLNK(st.st_mode):
                    raise Refused("raw SFDPS input contains a symlink")
                if stat.S_ISDIR(st.st_mode):
                    ndirectories += 1
                    if ndirectories > LIMITS["max_sfdps_directories"]:
                        raise Refused("raw SFDPS input exceeds frozen directory-count bound")
                    stack.append((path, depth + 1))
                elif stat.S_ISREG(st.st_mode):
                    nfiles += 1
                    nbytes += st.st_size
                    if st.st_nlink > 1:
                        raise Refused("raw SFDPS input contains a multiply-linked file")
                    if nfiles > max_files or nbytes > max_bytes:
                        raise Refused("raw SFDPS input exceeds frozen file/byte bounds")
                else:
                    raise Refused("raw SFDPS input contains a non-regular entry")
    return nfiles, nbytes


def _check_disk_floor(path: Path) -> None:
    usage = shutil.disk_usage(path)
    if usage.free < LIMITS["min_free_bytes"]:
        raise Refused("available disk space is below the frozen 64 GiB floor")


def _measure_output_tree(root: Path) -> tuple[int, int]:
    _guard().check(root)
    total = 0
    count = 0
    stack = [(root, 0)]
    directories = 1
    visited = set()
    while stack:
        directory, depth = stack.pop()
        dst = directory.stat(follow_symlinks=False)
        identity = (dst.st_dev, dst.st_ino)
        if identity in visited:
            raise Incomplete("child output tree repeats a directory inode")
        visited.add(identity)
        if depth > LIMITS["max_output_depth"]:
            raise Incomplete("child output tree exceeds its frozen directory-depth bound")
        with os.scandir(directory) as it:
            for entry in it:
                path = Path(entry.path)
                _guard().check(path)
                st = entry.stat(follow_symlinks=False)
                if stat.S_ISLNK(st.st_mode):
                    raise Incomplete("child output tree contains a symlink")
                if stat.S_ISDIR(st.st_mode):
                    directories += 1
                    if directories > LIMITS["max_output_directories"]:
                        raise Incomplete("child output tree exceeds its frozen directory-count bound")
                    stack.append((path, depth + 1))
                elif stat.S_ISREG(st.st_mode):
                    count += 1
                    total += st.st_size
                    if st.st_nlink != 1:
                        raise Incomplete("child output tree contains a multiply-linked file")
                    if count > LIMITS["max_output_files"] or total > LIMITS["max_child_output_bytes"]:
                        raise Incomplete("child output tree exceeded its frozen file/byte bound")
                else:
                    raise Incomplete("child output tree contains a special file")
    return count, total


def _new_private_dir(path: Path) -> None:
    _guard().check(path)
    if path.exists() or path.is_symlink():
        raise Refused("output directory must be new; no prior output may be reused")
    if not path.parent.is_dir():
        raise Refused("output parent directory must already exist")
    path.mkdir(mode=0o700)
    os.chmod(path, 0o700)
    _guard().check(path)


def _fsync_dir(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _atomic_json(path: Path, value: dict[str, Any]) -> None:
    raw = (json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    if len(raw) > 256 * 1024:
        raise Incomplete("run report exceeded its size limit")
    tmp = path.with_name(path.name + ".partial")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(tmp, flags, 0o600)
    try:
        with os.fdopen(fd, "wb", closefd=True) as f:
            f.write(raw)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
        _fsync_dir(path.parent)
    except BaseException:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise


def _private_output(path: Path, cap: int) -> tuple[int, str]:
    if path.is_symlink() or not path.is_file() or path.stat(follow_symlinks=False).st_nlink != 1:
        raise Incomplete(f"required output is missing or not a regular file: {path.name}")
    size, digest = _hash_file(path, cap)
    if size == 0:
        raise Incomplete(f"required output is empty: {path.name}")
    return size, digest


def _write_all(fd: int, data: bytes) -> None:
    offset = 0
    while offset < len(data):
        written = os.write(fd, data[offset:])
        if written <= 0:
            raise OSError("short write while preserving bounded child output")
        offset += written


def _count_text_rows(path: Path, max_rows: int) -> int:
    count = 0
    with _open_guarded(path) as f:
        while True:
            line = f.readline(MAX_LINE_BYTES + 1)
            if not line:
                break
            if len(line) > MAX_LINE_BYTES:
                raise Incomplete(f"{path.name}: line exceeds 64 KiB")
            count += 1
            if count > max_rows + 1:
                raise Incomplete(f"{path.name}: row count exceeds frozen bound")
    if count < 2:
        raise Incomplete(f"{path.name}: expected header and at least one row")
    return count - 1


def _kv_text(text: str, where: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in text.splitlines():
        key, sep, value = line.partition("=")
        if not sep or not key or key in result:
            raise Incomplete(f"{where} has a malformed or duplicate key")
        result[key] = value
    return result


def _validate_canon_rows(path: Path) -> int:
    with _open_guarded(path) as f:
        header = f.read(24)
        st = os.fstat(f.fileno())
    if len(header) != 24 or header[:8] != b"OATKROWS":
        raise Incomplete("canonical rows file has an invalid binary header")
    version = int.from_bytes(header[8:12], "little")
    row_bytes = int.from_bytes(header[12:16], "little")
    rows = int.from_bytes(header[16:24], "little")
    expected_bytes = {1: 85, 2: 96}.get(version)
    if expected_bytes != row_bytes or rows > LIMITS["max_output_rows"] or st.st_size != 24 + rows * row_bytes:
        raise Incomplete("canonical rows file version, count, or exact byte length is invalid")
    return rows


def _validate_canon_digest(path: Path, expected_rows: int) -> int:
    lines: list[str] = []
    with _open_guarded(path) as f:
        while True:
            raw = f.readline(MAX_LINE_BYTES + 1)
            if not raw:
                break
            if len(raw) > MAX_LINE_BYTES or len(lines) > 86_401:
                raise Incomplete("canonical digest exceeds its bounded line count or line length")
            try:
                lines.append(raw.decode("ascii", "strict").rstrip("\r\n"))
            except UnicodeDecodeError as exc:
                raise Incomplete("canonical digest is not ASCII") from exc
    if len(lines) < 2 or lines[0] != "# oa-track digest v2: second chain_hash second_hash rows; last line: end seconds rows chain":
        raise Incomplete("canonical digest header/schema is invalid")
    end = lines[-1].split()
    if len(end) != 4 or end[0] != "end" or not all(re.fullmatch(r"\d+", value, re.ASCII) for value in end[1:3]) or not HEX64.fullmatch(end[3]):
        raise Incomplete("canonical digest end record is invalid")
    seconds, rows = int(end[1]), int(end[2])
    if rows != expected_rows or seconds > 86_400 or len(lines) != seconds + 2:
        raise Incomplete("canonical digest second/row totals do not reconcile")
    previous: int | None = None
    summed_rows = 0
    lower_second = 1_796_018_400
    upper_second = 1_796_083_200
    for line in lines[1:-1]:
        fields = line.split()
        if len(fields) != 4 or not re.fullmatch(r"-?\d+", fields[0], re.ASCII) or not HEX64.fullmatch(fields[1]) or not HEX64.fullmatch(fields[2]) or not re.fullmatch(r"\d+", fields[3], re.ASCII):
            raise Incomplete("canonical digest contains a malformed second entry")
        second, row_count = int(fields[0]), int(fields[3])
        if second < lower_second or second >= upper_second or (previous is not None and second <= previous) or row_count <= 0:
            raise Incomplete("canonical digest seconds are out of the fixed half-open score interval or unordered")
        previous = second
        summed_rows += row_count
    if summed_rows != rows:
        raise Incomplete("canonical digest per-second rows do not sum to its end record")
    return seconds


def _parse_p010_report(text: str, arm: dict[str, Any], clip_sha: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in text.splitlines():
        if not line.startswith("P010_"):
            continue
        if "=" not in line:
            raise Incomplete("P-002 has a malformed P010 report line")
        key, value = line.split("=", 1)
        if key in values:
            raise Incomplete(f"P-002 repeats {key}")
        values[key] = value
    required = {
        "P010_MODE": "local-window-v1",
        "P010_STATUS": "measurement-complete",
        "P010_QUALIFICATION": "not-assessed; frozen-coverage-and-input-health-policy-required",
        "P010_WINDOW_PROFILE": "p010-zau-window-v1",
        "P010_ARM": "raw" if arm["publication_grid"] is None else "sfdps-v1",
        "P010_TIMING_PROFILE": arm["timing_profile"],
        "P010_TIMING_CALIBRATION_INPUTS": "versioned-constants-no-cadence-statistics" if arm["timing_profile"] == "fixed-10-30-120-v1" else ("full-set-A-before-warmup-filter-noncausal-C-replication-arm" if arm.get("tracker_role") == "replication" else "full-set-A-before-warmup-filter-noncausal-replication-arm"),
        "P010_FULL_CLIP_SPLIT": "once-before-set-A-filter",
        "P010_COHORT_IDENTITY_FORMAT": "domain-separated-sorted-addr-u32be-minute-i64be-truth-leg-u32be-v1",
        "P010_CLIP_SHA256": clip_sha,
        "P010_WARMUP_FROM": WARMUP_FROM,
        "P010_SCORE_FROM": SCORE_FROM,
        "P010_SCORE_TO": SCORE_TO,
        "P010_ERAM_AVAILABILITY_SCOPE": "post-P006-pairing-and-flight-selection; not-all-local-SFDPS-rows",
        "P010_PUBLIC_ELIGIBILITY": "one-unambiguous-set-B-truth-leg-per-address-at-whole-UTC-minute; valid-finite-position-and-velocity; speed-at-least-10-mps; actual-time-truth-leg-must-match-eligible-minute-leg; fixed-ZAU-box",
        "P010_SCORE_INTERVAL": "half-open",
        "P010_ELIGIBILITY_MINUTE_STEP_MS": "60000",
        "P010_ELIGIBILITY_MINUTE_ANCHOR": "score-from",
        "P010_ZAU_BOX_PROFILE": "zau-v1",
        "P010_ZAU_BOX_LAT_MIN_E7": "385000000",
        "P010_ZAU_BOX_LAT_MAX_E7": "465000000",
        "P010_ZAU_BOX_LON_MIN_E7": "-935000000",
        "P010_ZAU_BOX_LON_MAX_E7": "-835000000",
        "P010_CAP_STATUS": "within-limits",
        "P010_INTERPRETATION": "local-P010-comparison-only; not-P006-holdout; no-causal-or-safety-claim",
        "P010_PRIVATE_OUTPUTS": "p002.txt,samples.csv; do-not-publish-samples.csv",
    }
    for key, value in required.items():
        if values.get(key) != value:
            raise Incomplete(f"P-002 report does not satisfy {key}={value}")
    for key in (
        "P010_WARMUP_FROM_NS", "P010_SCORE_FROM_NS", "P010_SCORE_TO_NS",
        "P010_SFDPS_INPUT_BYTES", "P010_SFDPS_PREFLIGHT_INPUT_BYTES",
        "P010_ADDRESS_MINUTE_OPPORTUNITIES", "P010_NO_ACTIVE_LEG_ADDRESS_MINUTES",
        "P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES", "P010_AMBIGUOUS_ADDRESS_MINUTE_CANDIDATES",
        "P010_UNUSABLE_TRUTH_ADDRESS_MINUTES", "P010_NO_TRUTH_ADDRESS_MINUTES",
        "P010_ERAM_FINITE_PAIRED_POSITION_TIME_ROWS", "P010_ERAM_AVAILABLE_ADDRESS_MINUTES",
        "P010_ERAM_AVAILABILITY_DENOMINATOR", "P010_TRACKER_MEASUREMENTS_BEFORE_WINDOW_FILTER",
        "P010_TRACKER_MEASUREMENTS_AFTER_WARMUP_FILTER", "P010_CAP_MAX_ADDRESSES",
        "P010_CAP_MAX_ELIGIBLE_ADDRESS_MINUTES", "P010_CAP_MAX_SFDPS_INPUT_BYTES",
        "P010_ADSB_BAD_FRAMES", "P010_ADSB_DROPPED_RECORDS", "P010_ADSB_SEALED_DROPS",
        "P010_SFDPS_SEALED_DROPS",
    ):
        if key not in values or not re.fullmatch(r"\d+", values[key], re.ASCII):
            raise Incomplete(f"P-002 report has no valid integer {key}")
    if values["P010_WARMUP_FROM_NS"] != "1796014800000000000" or values["P010_SCORE_FROM_NS"] != "1796018400000000000" or values["P010_SCORE_TO_NS"] != "1796083200000000000":
        raise Incomplete("P-002 report nanosecond bounds differ from the frozen UTC window")
    if values["P010_SFDPS_INPUT_BYTES"] != values["P010_SFDPS_PREFLIGHT_INPUT_BYTES"]:
        raise Incomplete("P-002 loaded and preflight SFDPS byte identities differ")
    if int(values["P010_ERAM_FINITE_PAIRED_POSITION_TIME_ROWS"]) > LIMITS["max_output_rows"]:
        raise Incomplete("P-002 finite paired row count exceeds its output bound")
    partition = sum(int(values[key]) for key in ("P010_NO_ACTIVE_LEG_ADDRESS_MINUTES", "P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES", "P010_AMBIGUOUS_ADDRESS_MINUTE_CANDIDATES", "P010_UNUSABLE_TRUTH_ADDRESS_MINUTES", "P010_NO_TRUTH_ADDRESS_MINUTES"))
    if partition != int(values["P010_ADDRESS_MINUTE_OPPORTUNITIES"]):
        raise Incomplete("P-002 eligibility partition does not conserve address-minute opportunities")
    if int(values["P010_ERAM_AVAILABILITY_DENOMINATOR"]) != int(values["P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES"]):
        raise Incomplete("P-002 availability denominator differs from eligible address-minutes")
    if int(values["P010_ERAM_AVAILABLE_ADDRESS_MINUTES"]) > int(values["P010_ERAM_AVAILABILITY_DENOMINATOR"]):
        raise Incomplete("P-002 available address-minutes exceed the eligible denominator")
    try:
        availability_pct = float(values["P010_ERAM_AVAILABILITY_PERCENT"])
    except (KeyError, ValueError) as exc:
        raise Incomplete("P-002 availability percent is not numeric") from exc
    denominator = int(values["P010_ERAM_AVAILABILITY_DENOMINATOR"])
    expected_pct = 0.0 if denominator == 0 else int(values["P010_ERAM_AVAILABLE_ADDRESS_MINUTES"]) * 100.0 / denominator
    if not (0.0 <= availability_pct <= 100.0) or abs(availability_pct - expected_pct) > 0.00000051:
        raise Incomplete("P-002 availability percentage does not reconcile with its counts")
    if not HEX64.fullmatch(values.get("P010_SFDPS_AGGREGATE_SHA256", "")):
        raise Incomplete("P-002 report has no valid private SFDPS aggregate digest")
    for key in ("P010_PUBLIC_ELIGIBLE_COHORT_SHA256", "P010_ERAM_AVAILABLE_COHORT_SHA256"):
        if not HEX64.fullmatch(values.get(key, "")):
            raise Incomplete(f"P-002 report has no valid private {key} identity")
    frozen_caps = {
        "P010_CAP_MAX_ADDRESSES": "100000",
        "P010_CAP_MAX_ELIGIBLE_ADDRESS_MINUTES": "10000000",
        "P010_CAP_MAX_SFDPS_INPUT_BYTES": "64000000000",
    }
    if any(values.get(k) != v for k, v in frozen_caps.items()):
        raise Incomplete("P-002 cap literals differ from the P-010 v1 bounded profile")
    return values


def _validate_samples_csv(path: Path, arm: dict[str, Any]) -> int:
    expected = {
        "P010_MODE": "local-window-v1",
        "P010_ARM": "raw" if arm["publication_grid"] is None else "sfdps-v1",
        "P010_TIMING_PROFILE": arm["timing_profile"],
        "P010_WINDOW_PROFILE": "p010-zau-window-v1",
        "P010_WARMUP_FROM": WARMUP_FROM,
        "P010_SCORE_FROM": SCORE_FROM,
        "P010_SCORE_TO": SCORE_TO,
        "P010_ZAU_BOX_PROFILE": "zau-v1",
    }
    comments: dict[str, str] = {}
    rows = 0
    header_seen = False
    header = ["addr", "t_ms", "phase", "band", "common", "common_two_way"]
    for system in ("oa", "eram", "pt"):
        header.extend(f"{system}_{field}" for field in ("miss", "cross_m", "along_raw_m", "along_fit_m", "h_fit_m", "along_tol_m", "h_tol_m", "speed_err_mps", "track_err_deg", "row_dt_ms"))
    header.extend(("oa_coast", "oa_uncertain", "oa_reacquired"))
    with _open_guarded(path) as f:
        while True:
            raw = f.readline(MAX_LINE_BYTES + 1)
            if not raw:
                break
            if len(raw) > MAX_LINE_BYTES:
                raise Incomplete("samples.csv line exceeds 64 KiB")
            try:
                line = raw.decode("utf-8", "strict").rstrip("\r\n")
            except UnicodeDecodeError as exc:
                raise Incomplete("samples.csv is not UTF-8") from exc
            if line.startswith("# ") and not header_seen:
                if line[2:] == "DEVELOPMENT RUN — not a hold-out score":
                    continue
                if line[2:].startswith("OA_PUBLICATION_GRID="):
                    if arm["publication_grid"] != "sfdps-v1" or "OA_PUBLICATION_GRID" in comments:
                        raise Incomplete("samples.csv has an unexpected publication-grid stamp")
                    comments["OA_PUBLICATION_GRID"] = line[2:].split("=", 1)[1]
                    continue
                key, sep, value = line[2:].partition("=")
                if not sep or key in comments:
                    raise Incomplete("samples.csv has malformed or repeated P010 comments")
                comments[key] = value
                continue
            if line.startswith("#"):
                raise Incomplete("unexpected samples.csv comment")
            try:
                fields = next(csv.reader(io.StringIO(line)))
            except csv.Error as exc:
                raise Incomplete("samples.csv contains invalid CSV") from exc
            if not header_seen:
                if fields != header:
                    raise Incomplete("samples.csv header does not match the P-002 schema")
                header_seen = True
                continue
            if len(fields) != 39:
                raise Incomplete("samples.csv row does not match the 39-column P-002 schema")
            try:
                timestamp_ms = int(fields[1], 10)
            except ValueError as exc:
                raise Incomplete("samples.csv row has a non-integer timestamp") from exc
            if not 1_796_018_400_000 <= timestamp_ms < 1_796_083_200_000:
                raise Incomplete("samples.csv row is outside the fixed half-open scoring window")
            rows += 1
            if rows > LIMITS["max_output_rows"]:
                raise Incomplete("samples.csv row count exceeds its bound")
    if arm["publication_grid"] == "sfdps-v1":
        expected["OA_PUBLICATION_GRID"] = "sfdps-v1; OA tick positions rounded to 1 arcsecond then six decimal degrees and velocity components to 1 kt before pairing"
    if not header_seen or comments != expected:
        raise Incomplete("samples.csv is incomplete or its P010 profile stamps differ")
    return rows


def _approved_aggregate(text: str) -> list[str]:
    # CLAIM metric lines contain aggregate metrics only. Keep row/identity data and samples.csv
    # private; this allowlist deliberately excludes CLAIM run, which includes local input hashes.
    metrics = [line for line in text.splitlines() if line.startswith("CLAIM metric=")]
    expected = {"M1_median": "lower", "M5_speed_median": "lower", "M4t_300": "higher"}
    seen: set[str] = set()
    number = re.compile(r"(?:-?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?|NaN|inf|-inf)\Z", re.ASCII)
    for line in metrics:
        fields = line.split()
        if len(fields) != 10 or fields[0] != "CLAIM":
            raise Incomplete("P-002 aggregate CLAIM metric has an unexpected shape")
        names = ("metric", "better", "diff", "lo", "hi", "verdict", "floor", "oa3", "pt3")
        parsed: dict[str, str] = {}
        for token, name in zip(fields[1:], names):
            key, sep, value = token.partition("=")
            if not sep or key != name or not value:
                raise Incomplete("P-002 aggregate CLAIM metric fields are not canonical")
            parsed[key] = value
        metric = parsed["metric"]
        if metric not in expected or metric in seen or parsed["better"] != expected[metric]:
            raise Incomplete("P-002 aggregate metric ID or direction differs from the approved schema")
        if parsed["verdict"] not in {"oa", "eram", "none"} or parsed["floor"] not in {"0", "1"}:
            raise Incomplete("P-002 aggregate verdict/floor is invalid")
        if any(not number.fullmatch(parsed[k]) for k in ("diff", "lo", "hi", "oa3", "pt3")):
            raise Incomplete("P-002 aggregate contains a malformed metric value")
        lo, hi = float(parsed["lo"]), float(parsed["hi"])
        lower_is_better = expected[metric] == "lower"
        if not (math.isfinite(lo) and math.isfinite(hi)):
            if parsed["verdict"] != "none":
                raise Incomplete("P-002 non-finite interval cannot carry a directional verdict")
        else:
            if lo > hi:
                raise Incomplete("P-002 aggregate interval lower bound exceeds its upper bound")
            expected_verdict = (
                "oa" if (hi < 0.0 if lower_is_better else lo > 0.0)
                else "eram" if (lo > 0.0 if lower_is_better else hi < 0.0)
                else "none"
            )
            if parsed["verdict"] != expected_verdict:
                raise Incomplete("P-002 aggregate verdict contradicts its ordered interval")
        seen.add(metric)
    if seen != set(expected) or any(len(line.encode("utf-8")) > MAX_LINE_BYTES for line in metrics):
        raise Incomplete("P-002 aggregate CLAIM metric set is absent or incomplete")
    return metrics


def _aggregate_accounting(values: dict[str, str]) -> dict[str, Any]:
    # Counts disclose cohort/work totals only; addresses, per-address rows, input hashes and CSV
    # digests remain in the private run directory.
    return {
        "qualification": values["P010_QUALIFICATION"],
        "address_minute_opportunities": int(values["P010_ADDRESS_MINUTE_OPPORTUNITIES"]),
        "no_active_leg_address_minutes": int(values["P010_NO_ACTIVE_LEG_ADDRESS_MINUTES"]),
        "eligible_address_minutes": int(values["P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES"]),
        "ambiguous_address_minute_candidates": int(values["P010_AMBIGUOUS_ADDRESS_MINUTE_CANDIDATES"]),
        "unusable_truth_address_minutes": int(values["P010_UNUSABLE_TRUTH_ADDRESS_MINUTES"]),
        "no_truth_address_minutes": int(values["P010_NO_TRUTH_ADDRESS_MINUTES"]),
        "paired_position_time_rows": int(values["P010_ERAM_FINITE_PAIRED_POSITION_TIME_ROWS"]),
        "available_address_minutes": int(values["P010_ERAM_AVAILABLE_ADDRESS_MINUTES"]),
        "availability_denominator": int(values["P010_ERAM_AVAILABILITY_DENOMINATOR"]),
        "availability_percent": values["P010_ERAM_AVAILABILITY_PERCENT"],
        "tracker_measurements_before_window_filter": int(values["P010_TRACKER_MEASUREMENTS_BEFORE_WINDOW_FILTER"]),
        "tracker_measurements_after_warmup_filter": int(values["P010_TRACKER_MEASUREMENTS_AFTER_WARMUP_FILTER"]),
        "adsb_bad_frames": int(values["P010_ADSB_BAD_FRAMES"]),
        "adsb_dropped_records": int(values["P010_ADSB_DROPPED_RECORDS"]),
        "adsb_sealed_drops": int(values["P010_ADSB_SEALED_DROPS"]),
        "sfdps_sealed_drops": int(values["P010_SFDPS_SEALED_DROPS"]),
    }


def _reconcile_source_cohorts(arms: list[dict[str, Any]]) -> list[dict[str, Any]]:
    by_grid: dict[str, dict[str, dict[str, Any]]] = {}
    for arm in arms:
        grid = arm["publication_grid"] or "raw"
        role = arm["tracker_role"]
        if role in by_grid.setdefault(grid, {}):
            raise Incomplete("duplicate source role while reconciling private cohorts")
        by_grid[grid][role] = arm
    reconciled: list[dict[str, Any]] = []
    for grid in sorted(by_grid):
        pair = by_grid[grid]
        if set(pair) != {"candidate", "replication"}:
            raise Incomplete("candidate and C replication do not have a complete source-role pair")
        candidate = pair["candidate"]
        replication = pair["replication"]
        for key in ("clip_sha256_private", "sfdps_aggregate_sha256_private", "sfdps_input_bytes_private"):
            if candidate[key] != replication[key]:
                raise Incomplete(f"candidate/C paired arms differ in shared input identity: {key}")
        cand = candidate["private_cohorts"]
        repl = replication["private_cohorts"]
        if cand["eligible_count"] != repl["eligible_count"] or cand["eligible_sha256"] != repl["eligible_sha256"]:
            raise Incomplete("candidate/C eligibility sets differ; identical counts alone are insufficient")
        availability_equal = cand["available_count"] == repl["available_count"] and cand["available_sha256"] == repl["available_sha256"]
        reconciled.append({
            "publication_grid": grid,
            "shared_clip_and_sfdps_inputs": True,
            "public_eligibility_identity_equal": True,
            "public_eligible_address_minutes": cand["eligible_count"],
            "public_eligible_cohort_sha256_private": cand["eligible_sha256"],
            "eram_availability_identity_equal": availability_equal,
            "candidate_available_address_minutes": cand["available_count"],
            "candidate_eram_available_cohort_sha256_private": cand["available_sha256"],
            "replication_available_address_minutes": repl["available_count"],
            "replication_eram_available_cohort_sha256_private": repl["available_sha256"],
            "common_available_intersection_computed": False,
        })
    return reconciled


def _run_child(argv: list[str], cwd: Path, output_tree: Path, stdout_path: Path, stderr_path: Path, timeout_s: int) -> dict[str, Any]:
    if not argv or any(not isinstance(a, str) or "\x00" in a for a in argv):
        raise Refused("child argv contains an invalid argument")
    _guard().check(stdout_path)
    _guard().check(stderr_path)
    out_fd = os.open(stdout_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    err_fd = -1
    proc: subprocess.Popen[bytes] | None = None
    sel: selectors.BaseSelector | None = None
    try:
        err_fd = os.open(stderr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        proc = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, close_fds=True, env=CHILD_ENV)
        sel = selectors.DefaultSelector()
        streams = {proc.stdout: (out_fd, "stdout", 0), proc.stderr: (err_fd, "stderr", 0)}
        for pipe, (_, tag, _) in streams.items():
            if pipe is not None:
                os.set_blocking(pipe.fileno(), False)
                sel.register(pipe, selectors.EVENT_READ, tag)
        start = time.monotonic()
        timed_out = False
        log_overflow = False
        output_problem: str | None = None
        while proc.poll() is None or sel.get_map():
            left = timeout_s - (time.monotonic() - start)
            if left <= 0:
                timed_out = True
                break
            wait_for = min(0.25, left)
            events = sel.select(wait_for) if sel.get_map() else ()
            if not sel.get_map() and proc.poll() is None:
                time.sleep(wait_for)
            for key, _ in events:
                pipe = key.fileobj
                tag = key.data
                fd = out_fd if tag == "stdout" else err_fd
                raw = os.read(pipe.fileno(), 65536)
                if not raw:
                    sel.unregister(pipe)
                    pipe.close()
                    continue
                current = os.fstat(fd).st_size
                room = LIMITS["max_log_bytes"] - current
                if room < len(raw):
                    if room > 0:
                        _write_all(fd, raw[:room])
                    log_overflow = True
                    break
                _write_all(fd, raw)
            if log_overflow:
                break
            try:
                _check_disk_floor(output_tree)
                _measure_output_tree(output_tree)
            except (Incomplete, Refused) as exc:
                output_problem = str(exc)
                break
        if timed_out or log_overflow or output_problem is not None:
            _stop_owned(proc)
        else:
            proc.wait()
        status = proc.returncode
        if status is None:
            _stop_owned(proc)
            status = proc.returncode
        os.fsync(out_fd)
        os.fsync(err_fd)
        if not timed_out and not log_overflow and output_problem is None:
            _check_disk_floor(output_tree)
            _measure_output_tree(output_tree)
        result = {"argv": argv, "returncode": status, "timed_out": timed_out, "capture_limit_exceeded": log_overflow, "output_limit_or_type_error": output_problem, "elapsed_seconds": round(time.monotonic() - start, 3), "stdout_bytes": os.fstat(out_fd).st_size, "stderr_bytes": os.fstat(err_fd).st_size}
        if timed_out:
            raise Incomplete(f"child exceeded {timeout_s} seconds")
        if log_overflow:
            raise Incomplete("child log capture exceeded its frozen limit")
        if output_problem is not None:
            raise Incomplete(output_problem)
        if status != 0:
            raise Incomplete(f"child exited with status {status}")
        return result
    except BaseException:
        if proc is not None and proc.poll() is None:
            _stop_owned(proc)
        raise
    finally:
        if proc is not None:
            for pipe in (proc.stdout, proc.stderr):
                if pipe is not None and not pipe.closed:
                    pipe.close()
        for fd in (out_fd, err_fd):
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError:
                    pass
        if sel is not None:
            sel.close()


def _stop_owned(proc: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 3.0
    group_exists = True
    while time.monotonic() < deadline:
        proc.poll()  # reap the owned leader while allowing any owned descendants to terminate
        try:
            os.killpg(proc.pid, 0)
            group_exists = True
        except ProcessLookupError:
            group_exists = False
            break
        except PermissionError:
            group_exists = True
        time.sleep(0.05)
    if group_exists:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    if proc.poll() is None:
        proc.wait()


def _source_recheck(doc: dict[str, Any], roles: tuple[str, ...] = ("clipper", "candidate", "candidate_scorer", "replication_scorer")) -> None:
    source_roles: dict[str, dict[str, str]] = {}
    for role in roles:
        source = _build_source(doc, role)
        source_roles[source["tree"]] = source
    if "replication_scorer" in roles:
        source_roles[doc["replication"]["tree"]] = doc["replication"]
    for source in source_roles.values():
        _source_state(source["tree"], source["commit"])
    if "replication_scorer" in roles:
        _source_relation(doc)
    for role in roles:
        build = doc["builds"][role]
        try:
            meta_raw = _read_guarded(Path(build["metadata"]), MAX_BUILD_RECEIPT_BYTES)
            if _sha(meta_raw) != build["metadata_sha256"]:
                raise Incomplete(f"{role} build metadata changed during the run")
            _, digest = _hash_file(Path(build["binary"]), LIMITS["max_output_file_bytes"])
            if digest != build["binary_sha256"]:
                raise Incomplete(f"{role} build binary changed during the run")
        except Refused as exc:
            raise Incomplete(f"{role} build or compiled guard binding changed during the run: {exc}") from exc


def _prepare_out(args: argparse.Namespace) -> Path:
    out = Path(args.out)
    _new_private_dir(out)
    _check_disk_floor(out)
    return out.resolve(strict=True)


def _write_report(out: Path, report: dict[str, Any]) -> None:
    _atomic_json(out / "run-report.json", report)


def _public_run(args: argparse.Namespace) -> dict[str, Any]:
    doc, manifest_sha = _validate_manifest(Path(args.manifest))
    binaries = _preflight(doc, "public")
    archive = _verify_inventory(Path(args.archive_dir), doc)
    out = _prepare_out(args)
    report: dict[str, Any] = {"schema": REPORT_SCHEMA, "mode": "public", "status": "INCOMPLETE", "manifest_sha256": manifest_sha, "day": DAY, "candidate_commit": doc["candidate"]["commit"], "arms": [], "failure": None}
    _write_report(out, report)
    try:
        clip_dir = out / "clip"
        clip_dir.mkdir(mode=0o700)
        clip_path = clip_dir / "full-day.oaf"
        args_clip = [str(binaries["clipper"]), "--out", str(clip_path), "--box", ",".join(str(x) for x in doc["clip"]["box"]), "--from", ARCHIVE_FROM, "--to", ARCHIVE_TO, "--frame-ms", str(doc["clip"]["frame_ms"]), *map(str, archive)]
        _source_recheck(doc, ("clipper", "candidate"))
        clip_child = _run_child(args_clip, clip_dir, clip_dir, clip_dir / "clip.stdout", clip_dir / "clip.stderr", LIMITS["max_wall_seconds"])
        # Recheck the complete inventory after the clipper read before accepting any clip.
        if _verify_inventory(Path(args.archive_dir), doc) != archive:
            raise Incomplete("archive input paths changed while making the clip")
        _source_recheck(doc, ("clipper", "candidate"))
        clip_stderr = _read_guarded(clip_dir / "clip.stderr", LIMITS["max_log_bytes"]).decode("utf-8", "replace")
        if "archive: complete" not in clip_stderr.splitlines():
            raise Incomplete("oa-adsb-clip did not attest `archive: complete`")
        clip_size, clip_sha = _private_output(clip_path, LIMITS["max_output_file_bytes"])
        source_sidecar = clip_path.with_name(clip_path.name + ".source")
        sidecar_size, sidecar_sha = _private_output(source_sidecar, LIMITS["max_log_bytes"])
        report["clip"] = {"bytes": clip_size, "sha256": clip_sha, "source_sidecar_bytes": sidecar_size, "source_sidecar_sha256": sidecar_sha, "child": clip_child}
        _write_report(out, report)
        oa_root = out / "oa"
        oa_root.mkdir(mode=0o700)
        for arm in doc["public_arms"]:
            _check_disk_floor(out)
            arm_dir = oa_root / arm["name"]
            arm_dir.mkdir(mode=0o700)
            runner_dir = arm_dir / "tracker-output"
            runner_dir.mkdir(mode=0o700)
            _source_recheck(doc, ("clipper", "candidate"))
            cmd = [str(binaries["candidate"]), "--adsb", str(clip_path), "--out", str(runner_dir), "--public-window", "--no-score", "--no-compare", "--timing-profile", arm["timing_profile"], "--warmup-from", WARMUP_FROM, "--score-from", SCORE_FROM, "--score-to", SCORE_TO, "--digest", str(runner_dir / "canon.digest"), "--rows", str(runner_dir / "canon.rows")]
            child = _run_child(cmd, arm_dir, arm_dir, arm_dir / "stdout.log", arm_dir / "stderr.log", LIMITS["max_wall_seconds"])
            _source_recheck(doc, ("clipper", "candidate"))
            if _private_output(clip_path, LIMITS["max_output_file_bytes"]) != (clip_size, clip_sha):
                raise Incomplete("public clip changed during tracker invocation")
            digest_info = _private_output(runner_dir / "canon.digest", LIMITS["max_output_file_bytes"])
            rows_info = _private_output(runner_dir / "canon.rows", LIMITS["max_output_file_bytes"])
            rows_count = _validate_canon_rows(runner_dir / "canon.rows")
            seconds_count = _validate_canon_digest(runner_dir / "canon.digest", rows_count)
            window_info = _read_guarded(runner_dir / "public-window.txt", 64 * 1024).decode("utf-8", "strict")
            window_values = _kv_text(window_info, "public-window.txt")
            expected_window = {
                "format": "oa-track-public-window-v1",
                "profile": arm["timing_profile"],
                "input": "full_public_clip_loaded_and_split_once",
                "tracker_base_config": "DEFAULT_CONFIG",
                "tracker_non_timing_parameters": "unchanged_from_candidate_build",
                "warmup_from_utc": WARMUP_FROM,
                "score_from_utc": SCORE_FROM,
                "score_to_utc": SCORE_TO,
                "window_convention": "half_open",
                "canonical_interval": "[score_from_ns,score_to_ns)",
                "truth_scoring": "disabled",
                "wall_times": "omitted_from_deterministic_outputs",
            }
            if any(window_values.get(k) != v for k, v in expected_window.items()):
                raise Incomplete(f"public-window.txt for {arm['name']} does not bind the selected profile/window/split")
            try:
                metadata_rows = int(window_values["canonical_rows"])
                metadata_seconds = int(window_values["canonical_seconds"])
                for key in ("split_set_a_reports", "split_set_b_reports", "set_a_measurements_fed_to_tracker"):
                    int(window_values[key])
            except (KeyError, ValueError) as exc:
                raise Incomplete("public-window.txt lacks valid bounded accounting counts") from exc
            if arm["timing_profile"] == "fixed-10-30-120-v1":
                fixed_meta = {"profile_role": "fixed_development_profile", "timing_source": "versioned_constants", "timing_causality": "predeclared_constants_no_interval_calibration", "expected_interval_ns": "10000000000", "init_interval_ns": "30000000000", "drop_after_ns": "120000000000", "interval_stats_scope": "not_computed", "interval_n": "not_computed", "interval_p50_ns": "not_computed", "interval_p90_ns": "not_computed", "interval_p99_ns": "not_computed"}
                if any(window_values.get(k) != v for k, v in fixed_meta.items()):
                    raise Incomplete("fixed public timing profile metadata differs from its versioned constants")
            else:
                legacy_meta = {"profile_role": "secondary_noncausal_comparison", "interval_stats_scope": "full_set_a_before_warmup_filter", "timing_causality": "noncausal_full_set_a_interval_calibration_before_warmup_filter"}
                if any(window_values.get(k) != v for k, v in legacy_meta.items()) or window_values.get("timing_source") not in {"measured_intervals", "legacy_one_second_fallback"}:
                    raise Incomplete("legacy public timing metadata does not disclose its full-set-A calibration")
            if metadata_rows != rows_count or metadata_seconds != seconds_count:
                raise Incomplete("public-window accounting differs from canonical output files")
            if "elapsed" in window_info.lower() or "wall_seconds" in window_info.lower():
                raise Incomplete("public-window.txt unexpectedly contains elapsed metadata")
            row = {"name": arm["name"], "timing_profile": arm["timing_profile"], "rows": rows_count, "seconds": seconds_count, "digest": {"bytes": digest_info[0], "sha256": digest_info[1]}, "canonical_rows": {"bytes": rows_info[0], "sha256": rows_info[1]}, "profile_summary_sha256": _sha(window_info.encode("utf-8")), "child": child}
            report["arms"].append(row)
            _write_report(out, report)
        _source_recheck(doc, ("clipper", "candidate"))
        public_summary = {"schema": PUBLIC_SCHEMA, "status": "COMPLETE", "day": DAY, "clip_sha256": clip_sha, "oa_arms": [{k: a[k] for k in ("name", "timing_profile", "rows", "seconds", "digest", "canonical_rows", "profile_summary_sha256")} for a in report["arms"]]}
        _atomic_json(out / "public-summary.json", public_summary)
        report["status"] = "COMPLETE"
        report["public_summary_sha256"] = _sha((json.dumps(public_summary, ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"))
        _write_report(out, report)
        return {"status": "COMPLETE", "summary": str(out / "public-summary.json"), "arms": len(report["arms"])}
    except BaseException as exc:
        report["failure"] = {"type": type(exc).__name__, "message": str(exc)[:2048]}
        report["status"] = "INCOMPLETE"
        try:
            _write_report(out, report)
        except BaseException:
            pass
        raise


def _verify_public_run(public_dir: Path, doc: dict[str, Any], manifest_sha: str) -> tuple[Path, str, dict[str, Any]]:
    _guard().check(public_dir)
    report_path = public_dir / "run-report.json"
    raw = _read_guarded(report_path, 256 * 1024)
    try:
        report = json.loads(raw.decode("utf-8"), object_pairs_hook=_object_no_duplicates)
    except RecursionError as exc:
        raise Refused("public run report exceeds the supported JSON nesting depth") from exc
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise Refused(f"public run report is invalid: {exc}") from exc
    if report.get("schema") != REPORT_SCHEMA or report.get("mode") != "public" or report.get("status") != "COMPLETE" or report.get("manifest_sha256") != manifest_sha:
        raise Refused("public run report is not a complete run for this frozen manifest")
    if report.get("candidate_commit") != doc["candidate"]["commit"] or not isinstance(report.get("clip"), dict):
        raise Refused("public run report candidate/clip binding mismatch")
    clip_path = public_dir / "clip" / "full-day.oaf"
    _, sha = _hash_file(clip_path, LIMITS["max_output_file_bytes"])
    if sha != report["clip"].get("sha256"):
        raise Refused("public run clip has changed since its complete report")
    sidecar = public_dir / "clip" / "full-day.oaf.source"
    sidecar_size, sidecar_sha = _private_output(sidecar, LIMITS["max_log_bytes"])
    if sidecar_size != report["clip"].get("source_sidecar_bytes") or sidecar_sha != report["clip"].get("source_sidecar_sha256"):
        raise Refused("public run clip source sidecar changed since its complete report")
    summary_path = public_dir / "public-summary.json"
    summary_raw = _read_guarded(summary_path, 256 * 1024)
    if _sha(summary_raw) != report.get("public_summary_sha256"):
        raise Refused("public summary bytes differ from the completed run report")
    try:
        summary = json.loads(summary_raw.decode("utf-8"), object_pairs_hook=_object_no_duplicates)
    except RecursionError as exc:
        raise Refused("public summary exceeds the supported JSON nesting depth") from exc
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise Refused(f"public summary is invalid: {exc}") from exc
    if not isinstance(summary, dict) or set(summary) != {"schema", "status", "day", "clip_sha256", "oa_arms"}:
        raise Refused("public summary has an unexpected field set")
    if summary != {"schema": PUBLIC_SCHEMA, "status": "COMPLETE", "day": DAY, "clip_sha256": sha, "oa_arms": [{k: a.get(k) for k in ("name", "timing_profile", "rows", "seconds", "digest", "canonical_rows", "profile_summary_sha256")} for a in report.get("arms", [])]}:
        raise Refused("public summary contents do not reconcile to the run report")
    if len(report.get("arms", [])) != len(doc["public_arms"]):
        raise Refused("public run does not contain every manifest-declared OA arm")
    for arm_config, arm in zip(doc["public_arms"], report["arms"]):
        if arm.get("name") != arm_config["name"] or arm.get("timing_profile") != arm_config["timing_profile"]:
            raise Refused("public run arm order/profile differs from the frozen manifest")
        arm_dir = public_dir / "oa" / arm_config["name"] / "tracker-output"
        for field, filename in (("digest", "canon.digest"), ("canonical_rows", "canon.rows")):
            item = arm.get(field)
            size, digest = _private_output(arm_dir / filename, LIMITS["max_output_file_bytes"])
            if not isinstance(item, dict) or size != item.get("bytes") or digest != item.get("sha256"):
                raise Refused(f"public run {filename} changed since its complete report")
        rows = _validate_canon_rows(arm_dir / "canon.rows")
        seconds = _validate_canon_digest(arm_dir / "canon.digest", rows)
        metadata = _read_guarded(arm_dir / "public-window.txt", 64 * 1024).decode("utf-8", "strict")
        if _sha(metadata.encode("utf-8")) != arm.get("profile_summary_sha256"):
            raise Refused("public run profile metadata changed since its complete report")
        values = _kv_text(metadata, "public-window.txt")
        if values.get("profile") != arm_config["timing_profile"] or values.get("input") != "full_public_clip_loaded_and_split_once" or values.get("canonical_rows") != str(rows) or values.get("canonical_seconds") != str(seconds):
            raise Refused("public run output accounting does not reconcile")
    return clip_path.resolve(strict=True), sha, summary


def _compare_run(args: argparse.Namespace) -> dict[str, Any]:
    doc, manifest_sha = _validate_manifest(Path(args.manifest))
    binaries = _preflight(doc, "compare")
    if not doc["compare_arms"]:
        raise Refused("compare mode requires at least one frozen compare arm")
    archive_dir = Path(args.archive_dir)
    _verify_inventory(archive_dir, doc)
    sfdps = Path(args.sfdps_dir)
    sfdps_files, sfdps_bytes = _measure_dir(sfdps, LIMITS["max_sfdps_files"], LIMITS["max_sfdps_bytes"])
    public_dir = Path(args.public_run_dir)
    clip_path, clip_sha, public_summary = _verify_public_run(public_dir, doc, manifest_sha)
    out = _prepare_out(args)
    report: dict[str, Any] = {"schema": REPORT_SCHEMA, "mode": "private-compare", "status": "INCOMPLETE", "qualification": "not-assessed; frozen-coverage-and-input-health-policy-required", "manifest_sha256": manifest_sha, "candidate_tracker_commit": doc["candidate"]["commit"], "replication_tracker_commit": doc["replication"]["commit"], "replication_scorer_commit": doc["replication_scorer"]["commit"], "replication_scorer_patch": {"base_commit": doc["replication_scorer_relation"]["base_commit"], "scorer_commit": doc["replication_scorer_relation"]["scorer_commit"], "changed_paths": doc["replication_scorer_relation"]["changed_paths"], "delta_sha256_private": doc["replication_scorer_relation"]["delta_sha256"], "review_sha256_private": doc["replication_scorer_relation"]["review_sha256"]}, "public_clip_sha256": clip_sha, "sfdps_files": sfdps_files, "sfdps_bytes": sfdps_bytes, "arms": [], "cohort_reconciliation": [], "failure": None}
    _write_report(out, report)
    try:
        frozen_sfdps_sha: str | None = None
        frozen_sfdps_bytes: int | None = None
        for arm in doc["compare_arms"]:
            _check_disk_floor(out)
            private_root = out / "private"
            if not private_root.exists():
                private_root.mkdir(mode=0o700)
            arm_dir = private_root / arm["name"]
            arm_dir.mkdir(mode=0o700)
            scorer_out = arm_dir / "p002-output"
            scorer_out.mkdir(mode=0o700)
            ledger = arm_dir / "ledger.txt"
            fd = os.open(ledger, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
            os.fsync(fd)
            os.close(fd)
            tracker_role = arm["tracker_role"]
            build_role = "candidate_scorer" if tracker_role == "candidate" else "replication_scorer"
            tracker_source = doc["candidate"] if tracker_role == "candidate" else doc["replication"]
            scorer_source = doc["candidate"] if tracker_role == "candidate" else doc["replication_scorer"]
            scorer_binary = binaries[build_role]
            scorer_binary_sha = doc["builds"][build_role]["binary_sha256"]
            _source_recheck(doc)
            cmd = [str(scorer_binary), "--p010-local-window", "--adsb", str(clip_path), "--sfdps-dir", str(sfdps.resolve(strict=True)), "--out", str(scorer_out), "--warmup-from", WARMUP_FROM, "--score-from", SCORE_FROM, "--score-to", SCORE_TO, "--zau-box-profile", "zau-v1", "--development", "--ledger", str(ledger), "--timing-profile", arm["timing_profile"]]
            if arm["publication_grid"] == "sfdps-v1":
                cmd.extend(["--oa-publication-grid", "sfdps-v1"])
            child = _run_child(cmd, arm_dir, arm_dir, arm_dir / "stdout.log", arm_dir / "stderr.log", LIMITS["max_wall_seconds"])
            _source_recheck(doc)
            p002_path = scorer_out / "p002.txt"
            csv_path = scorer_out / "samples.csv"
            p002_size, p002_sha = _private_output(p002_path, LIMITS["max_log_bytes"])
            csv_size, csv_sha = _private_output(csv_path, LIMITS["max_output_file_bytes"])
            p002_text = _read_guarded(p002_path, LIMITS["max_log_bytes"]).decode("utf-8", "strict")
            csv_rows = _validate_samples_csv(csv_path, arm)
            p010_values = _parse_p010_report(p002_text, arm, clip_sha)
            if int(p010_values["P010_SFDPS_PREFLIGHT_INPUT_BYTES"]) > sfdps_bytes:
                raise Incomplete("P-002 SFDPS preflight byte count exceeds the guarded input tree")
            if int(p010_values["P010_SFDPS_INPUT_BYTES"]) != int(p010_values["P010_SFDPS_PREFLIGHT_INPUT_BYTES"]):
                raise Incomplete("P-002 loaded SFDPS bytes differ from its supported-input preflight")
            arm_sfdps_sha = p010_values["P010_SFDPS_AGGREGATE_SHA256"]
            arm_sfdps_bytes = int(p010_values["P010_SFDPS_INPUT_BYTES"])
            if frozen_sfdps_sha is not None and arm_sfdps_sha != frozen_sfdps_sha:
                raise Incomplete("P-002 SFDPS aggregate digest changed between comparison arms")
            if frozen_sfdps_bytes is not None and arm_sfdps_bytes != frozen_sfdps_bytes:
                raise Incomplete("P-002 SFDPS parser byte count changed between comparison arms")
            frozen_sfdps_sha, frozen_sfdps_bytes = arm_sfdps_sha, arm_sfdps_bytes
            row = {"name": arm["name"], "tracker_role": tracker_role, "tracker_commit": tracker_source["commit"], "scorer_commit": scorer_source["commit"], "scorer_binary_sha256_private": scorer_binary_sha, "timing_profile": arm["timing_profile"], "publication_grid": arm["publication_grid"], "p002_bytes": p002_size, "p002_sha256_private": p002_sha, "samples_csv_bytes": csv_size, "samples_csv_sha256_private": csv_sha, "samples_csv_rows": csv_rows, "clip_sha256_private": clip_sha, "sfdps_aggregate_sha256_private": arm_sfdps_sha, "sfdps_input_bytes_private": arm_sfdps_bytes, "private_cohorts": {"eligible_count": int(p010_values["P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES"]), "eligible_sha256": p010_values["P010_PUBLIC_ELIGIBLE_COHORT_SHA256"], "available_count": int(p010_values["P010_ERAM_AVAILABLE_ADDRESS_MINUTES"]), "available_sha256": p010_values["P010_ERAM_AVAILABLE_COHORT_SHA256"]}, "accounting": _aggregate_accounting(p010_values), "aggregate": _approved_aggregate(p002_text), "child": child}
            report["arms"].append(row)
            _write_report(out, report)
        report["cohort_reconciliation"] = _reconcile_source_cohorts(report["arms"])
        _source_recheck(doc)
        aggregate = {
            "schema": AGGREGATE_SCHEMA,
            "day": DAY,
            "measurement_status": "complete",
            "qualification": "not-assessed; frozen-coverage-and-input-health-policy-required",
            "window": {"warmup_from": WARMUP_FROM, "score_from": SCORE_FROM, "score_to": SCORE_TO, "interval": "half-open"},
            "arms": [{"name": arm["name"], "tracker_role": arm["tracker_role"], "timing_profile": arm["timing_profile"], "publication_grid": arm["publication_grid"], "accounting": arm["accounting"], "claims": arm["aggregate"]} for arm in report["arms"]],
            "bindings": {
                "driver_sha256": doc["driver_sha256"],
                "manifest_sha256": manifest_sha,
                "candidate_commit": doc["candidate"]["commit"],
                "replication_commit": doc["replication"]["commit"],
                "replication_scorer_commit": doc["replication_scorer"]["commit"],
                "replication_scorer_patch_sha256": doc["replication_scorer_relation"]["delta_sha256"],
                "build_binary_sha256": {role: doc["builds"][role]["binary_sha256"] for role in BUILD_ROLES},
                "public_archive_inventory_sha256": doc["release"]["inventory_sha256"],
                "public_clip_sha256": clip_sha,
                "public_oa_outputs": [
                    {key: arm[key] for key in ("name", "timing_profile", "rows", "seconds", "digest", "canonical_rows", "profile_summary_sha256")}
                    for arm in public_summary["oa_arms"]
                ],
                "compare_profiles": [
                    {key: arm[key] for key in ("name", "tracker_role", "timing_profile", "publication_grid")}
                    for arm in report["arms"]
                ],
                "health_eligibility": "unknown-not-assessed",
                "disclosure_eligibility": "unknown-not-assessed",
            },
            "cohort_comparability": [{"publication_grid": item["publication_grid"], "public_eligibility_identity_equal": item["public_eligibility_identity_equal"], "eram_availability_identity_equal": item["eram_availability_identity_equal"], "common_available_intersection_computed": item["common_available_intersection_computed"]} for item in report["cohort_reconciliation"]],
            "availability_interpretation": "candidate-and-C-ERAM-available-cohorts-have-identical-private-identities" if all(item["eram_availability_identity_equal"] for item in report["cohort_reconciliation"]) else "availability-cohorts-differ; private identities retained; no common available intersection computed, so do not interpret as a common-availability matched subset",
            "interpretation": "authorized-aggregate-only; local-P010-comparison; not-P006-holdout; no-causal-or-safety-claim",
        }
        _atomic_json(out / "aggregate-summary.json", aggregate)
        report["aggregate_summary_sha256"] = _sha((json.dumps(aggregate, ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"))
        report["status"] = "MEASUREMENT_COMPLETE"
        _write_report(out, report)
        return {"status": "MEASUREMENT_COMPLETE", "qualification": report["qualification"], "private_report": str(out / "run-report.json"), "aggregate_summary": str(out / "aggregate-summary.json"), "arms": len(report["arms"])}
    except BaseException as exc:
        report["failure"] = {"type": type(exc).__name__, "message": str(exc)[:2048]}
        report["status"] = "INCOMPLETE"
        try:
            _write_report(out, report)
        except BaseException:
            pass
        raise


def _read_cross_capture_summary(path: Path) -> dict[str, Any]:
    raw = _read_guarded(path, 512 * 1024)
    try:
        summary = json.loads(raw.decode("utf-8", "strict"), object_pairs_hook=_object_no_duplicates, parse_constant=_reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise Refused(f"cross-capture input is not strict bounded UTF-8 JSON: {exc}") from exc
    expected = {"schema", "day", "measurement_status", "qualification", "window", "arms", "bindings", "cohort_comparability", "availability_interpretation", "interpretation"}
    if not isinstance(summary, dict) or set(summary) != expected:
        raise Refused("cross-capture input has an unexpected aggregate-summary field set")
    if summary["schema"] != AGGREGATE_SCHEMA or summary["day"] != DAY or summary["measurement_status"] != "complete":
        raise Refused("cross-capture input is not a complete summary from this P-010 schema and day")
    if summary["qualification"] != "not-assessed; frozen-coverage-and-input-health-policy-required":
        raise Refused("cross-capture input has an unexpected qualification state")
    if summary["window"] != {"warmup_from": WARMUP_FROM, "score_from": SCORE_FROM, "score_to": SCORE_TO, "interval": "half-open"}:
        raise Refused("cross-capture window binding does not match the fixed P-010 window")
    bind = summary["bindings"]
    bind_keys = {"driver_sha256", "manifest_sha256", "candidate_commit", "replication_commit", "replication_scorer_commit", "replication_scorer_patch_sha256", "build_binary_sha256", "public_archive_inventory_sha256", "public_clip_sha256", "public_oa_outputs", "compare_profiles", "health_eligibility", "disclosure_eligibility"}
    if not isinstance(bind, dict) or set(bind) != bind_keys:
        raise Refused("cross-capture input has an unexpected source/run binding field set")
    driver_bytes = _read_guarded(SCRIPT_PATH, 2 * 1024 * 1024)
    if bind["driver_sha256"] != _sha(driver_bytes):
        raise Refused("cross-capture driver source differs from this comparator")
    for key in ("driver_sha256", "manifest_sha256", "replication_scorer_patch_sha256", "public_archive_inventory_sha256", "public_clip_sha256"):
        if not isinstance(bind[key], str) or not HEX64.fullmatch(bind[key]):
            raise Refused(f"cross-capture binding {key} is not a lowercase SHA-256")
    for key in ("candidate_commit", "replication_commit", "replication_scorer_commit"):
        if not isinstance(bind[key], str) or not HEX40.fullmatch(bind[key]):
            raise Refused(f"cross-capture binding {key} is not a lowercase commit ID")
    if not isinstance(bind["build_binary_sha256"], dict) or set(bind["build_binary_sha256"]) != set(BUILD_ROLES):
        raise Refused("cross-capture build bindings do not contain the exact P-010 build roles")
    if any(not isinstance(value, str) or not HEX64.fullmatch(value) for value in bind["build_binary_sha256"].values()):
        raise Refused("cross-capture build binding has a malformed binary hash")
    if bind["health_eligibility"] != "unknown-not-assessed" or bind["disclosure_eligibility"] != "unknown-not-assessed":
        raise Refused("cross-capture input must retain unresolved health and disclosure eligibility")
    if not isinstance(bind["public_oa_outputs"], list) or not 1 <= len(bind["public_oa_outputs"]) <= 2:
        raise Refused("cross-capture input lacks public tracker output bindings")
    public_output_keys = {"name", "timing_profile", "rows", "seconds", "digest", "canonical_rows", "profile_summary_sha256"}
    for output in bind["public_oa_outputs"]:
        if not isinstance(output, dict) or set(output) != public_output_keys:
            raise Refused("cross-capture public tracker output has an unexpected field set")
        if not isinstance(output["name"], str) or output["timing_profile"] not in TIMING_PROFILES:
            raise Refused("cross-capture public tracker output has an invalid name/profile")
        for key in ("rows", "seconds"):
            if not isinstance(output[key], int) or isinstance(output[key], bool) or output[key] < 0:
                raise Refused("cross-capture public tracker output count is invalid")
        for key in ("digest", "canonical_rows"):
            item = output[key]
            if not isinstance(item, dict) or set(item) != {"bytes", "sha256"} or not isinstance(item["bytes"], int) or isinstance(item["bytes"], bool) or item["bytes"] < 0 or not isinstance(item["sha256"], str) or not HEX64.fullmatch(item["sha256"]):
                raise Refused("cross-capture public tracker output hash binding is invalid")
        if not isinstance(output["profile_summary_sha256"], str) or not HEX64.fullmatch(output["profile_summary_sha256"]):
            raise Refused("cross-capture public profile metadata hash is invalid")
    if not isinstance(bind["compare_profiles"], list) or not 2 <= len(bind["compare_profiles"]) <= 4:
        raise Refused("cross-capture input lacks compare-arm profiles")
    profile_keys = {"name", "tracker_role", "timing_profile", "publication_grid"}
    for profile in bind["compare_profiles"]:
        if not isinstance(profile, dict) or set(profile) != profile_keys or profile["tracker_role"] not in {"candidate", "replication"} or profile["timing_profile"] not in TIMING_PROFILES or profile["publication_grid"] not in GRID_PROFILES:
            raise Refused("cross-capture compare-arm profile binding is invalid")
    if not isinstance(summary["arms"], list) or not 2 <= len(summary["arms"]) <= 4:
        raise Refused("cross-capture input has no aggregate metric arms")
    arm_keys = {"name", "tracker_role", "timing_profile", "publication_grid", "accounting", "claims"}
    accounting_keys = {"qualification", "address_minute_opportunities", "no_active_leg_address_minutes", "eligible_address_minutes", "ambiguous_address_minute_candidates", "unusable_truth_address_minutes", "no_truth_address_minutes", "paired_position_time_rows", "available_address_minutes", "availability_denominator", "availability_percent", "tracker_measurements_before_window_filter", "tracker_measurements_after_warmup_filter", "adsb_bad_frames", "adsb_dropped_records", "adsb_sealed_drops", "sfdps_sealed_drops"}
    expected_profiles: list[dict[str, Any]] = []
    seen_arms: set[str] = set()
    for arm in summary["arms"]:
        if not isinstance(arm, dict) or set(arm) != arm_keys or arm["tracker_role"] not in {"candidate", "replication"} or arm["timing_profile"] not in TIMING_PROFILES or arm["publication_grid"] not in GRID_PROFILES:
            raise Refused("cross-capture aggregate arm has an invalid schema or profile")
        if not isinstance(arm["name"], str) or not ARM_NAME.fullmatch(arm["name"]) or arm["name"] in seen_arms:
            raise Refused("cross-capture aggregate arm names are invalid or duplicated")
        seen_arms.add(arm["name"])
        expected_profiles.append({key: arm[key] for key in ("name", "tracker_role", "timing_profile", "publication_grid")})
        accounting = arm["accounting"]
        if not isinstance(accounting, dict) or set(accounting) != accounting_keys or accounting["qualification"] != summary["qualification"]:
            raise Refused("cross-capture arm accounting has an unexpected aggregate-only schema")
        for key, value in accounting.items():
            if key == "qualification":
                continue
            if key == "availability_percent":
                try:
                    percent = float(value)
                except (TypeError, ValueError) as exc:
                    raise Refused("cross-capture arm availability percentage is invalid") from exc
                if not math.isfinite(percent) or not 0.0 <= percent <= 100.0:
                    raise Refused("cross-capture arm availability percentage is outside [0,100]")
            elif not isinstance(value, int) or isinstance(value, bool) or value < 0:
                raise Refused("cross-capture arm accounting contains a non-count or negative value")
        if not isinstance(arm["claims"], list) or len(arm["claims"]) != 3 or any(not isinstance(line, str) for line in arm["claims"]):
            raise Refused("cross-capture aggregate arm claims are malformed")
        try:
            normalized = _approved_aggregate("\n".join(arm["claims"]))
        except Incomplete as exc:
            raise Refused(f"cross-capture aggregate claims fail validation: {exc}") from exc
        if normalized != arm["claims"]:
            raise Refused("cross-capture claims are not canonical approved aggregate lines")
    if bind["compare_profiles"] != expected_profiles:
        raise Refused("cross-capture compare-profile binding differs from aggregate arm metadata")
    cohort_keys = {"publication_grid", "public_eligibility_identity_equal", "eram_availability_identity_equal", "common_available_intersection_computed"}
    if not isinstance(summary["cohort_comparability"], list) or len(summary["cohort_comparability"]) > 2:
        raise Refused("cross-capture cohort status is malformed")
    for item in summary["cohort_comparability"]:
        if not isinstance(item, dict) or set(item) != cohort_keys or item["publication_grid"] not in {"raw", "sfdps-v1"} or any(not isinstance(item[key], bool) for key in cohort_keys - {"publication_grid"}):
            raise Refused("cross-capture cohort status contains a non-aggregate or malformed value")
    return summary


def _claim_values(lines: list[str]) -> dict[str, dict[str, float]]:
    values: dict[str, dict[str, float]] = {}
    for line in lines:
        fields = line.split()
        parsed = {token.split("=", 1)[0]: token.split("=", 1)[1] for token in fields[1:]}
        metric = parsed["metric"]
        vals = {key: float(parsed[key]) for key in ("diff", "lo", "hi")}
        if not all(math.isfinite(value) for value in vals.values()) or vals["lo"] > vals["hi"]:
            raise Refused(f"cross-capture metric {metric} has non-finite or reversed interval data")
        values[metric] = vals
    return values


def _sign(value: float) -> int:
    return 1 if value > 0.0 else -1 if value < 0.0 else 0


def _compare_capture_summaries(project: dict[str, Any], outsider: dict[str, Any], arm_name: str) -> dict[str, Any]:
    # Do not treat per-run provenance, equal counts, or participant-held cohort hashes as
    # cross-participant matched-cohort evidence. No approved private attestation is an input here.
    pb, ob = project["bindings"], outsider["bindings"]
    # Build hashes remain required per-summary provenance, but independent participants may
    # produce different executable bytes for the same pinned sources on different platforms.
    shared = ("driver_sha256", "candidate_commit", "replication_commit", "replication_scorer_commit", "replication_scorer_patch_sha256", "public_archive_inventory_sha256", "public_clip_sha256", "public_oa_outputs", "compare_profiles", "health_eligibility", "disclosure_eligibility")
    mismatched = [key for key in shared if pb[key] != ob[key]]
    if mismatched:
        raise Refused("participant summaries differ in shared source/public-run/window/profile/health bindings: " + ", ".join(mismatched))
    selected: list[dict[str, Any]] = []
    for label, summary in (("project", project), ("outsider", outsider)):
        matches = [arm for arm in summary["arms"] if arm["name"] == arm_name]
        if len(matches) != 1:
            raise Refused(f"{label} summary must contain exactly one selected arm {arm_name!r}")
        item = matches[0]
        if item["tracker_role"] != "candidate" or item["timing_profile"] != "fixed-10-30-120-v1":
            raise Refused("selected comparison arm must bind the fixed candidate tracker profile")
        selected.append(item)
    left, right = selected
    if (left["timing_profile"], left["publication_grid"]) != (right["timing_profile"], right["publication_grid"]):
        raise Refused("selected participant arm profiles differ")
    left_values, right_values = _claim_values(left["claims"]), _claim_values(right["claims"])
    expected = {"M1_median": (0.25, 0.05), "M5_speed_median": (0.03, 0.05), "M4t_300": (0.005, 0.0)}
    if set(left_values) != set(expected) or set(right_values) != set(expected):
        raise Refused("selected participant arms do not carry the exact three primary metrics")
    metrics: dict[str, Any] = {}
    for metric, (absolute_floor, relative_fraction) in expected.items():
        p, o = left_values[metric], right_values[metric]
        delta = o["diff"] - p["diff"]
        bound = max(absolute_floor, relative_fraction * abs(p["diff"]))
        project_excludes_zero = p["lo"] > 0.0 or p["hi"] < 0.0
        outsider_excludes_zero = o["lo"] > 0.0 or o["hi"] < 0.0
        project_ci_sign = _sign(p["lo"]) if project_excludes_zero and p["lo"] > 0.0 else _sign(p["hi"]) if project_excludes_zero else 0
        outsider_ci_sign = _sign(o["lo"]) if outsider_excludes_zero and o["lo"] > 0.0 else _sign(o["hi"]) if outsider_excludes_zero else 0
        sign_same = _sign(p["diff"]) == _sign(o["diff"])
        excludes_zero_same = project_excludes_zero == outsider_excludes_zero
        ci_direction_same = not project_excludes_zero or not outsider_excludes_zero or project_ci_sign == outsider_ci_sign
        metrics[metric] = {
            "project_delta": p["diff"], "outsider_delta": o["diff"], "outsider_minus_project": delta,
            "proposed_bound": bound, "within_proposed_bound": abs(delta) <= bound,
            "project_interval": [p["lo"], p["hi"]], "outsider_interval": [o["lo"], o["hi"]],
            "point_sign_same": sign_same, "interval_excludes_zero_same": excludes_zero_same,
            "interval_direction_same_when_excluding_zero": ci_direction_same,
            "descriptive_alignment": sign_same and excludes_zero_same and ci_direction_same,
            "statistical_verdict": "not-assessed; exact statistic and relative-scale anchor await approval",
        }
    numeric_bounds_all = all(item["within_proposed_bound"] for item in metrics.values())
    descriptive_alignment_all = all(item["descriptive_alignment"] for item in metrics.values())
    return {
        "schema": CROSS_CAPTURE_SCHEMA, "day": DAY, "status": "COMPARISON_COMPLETE",
        "selected_arm": {"name": arm_name, "tracker_role": "candidate", "timing_profile": "fixed-10-30-120-v1", "publication_grid": left["publication_grid"]},
        "shared_bindings_match": True,
        "numeric_bounds": "within-proposed-bounds" if numeric_bounds_all else "outside-proposed-bounds",
        "descriptive_alignment": "aligned" if descriptive_alignment_all else "not-aligned",
        "metrics": metrics,
        "prediction_hit_miss": "not-assessed-separately",
        "matched_cohort_checked": "not-assessed",
        "health_eligibility": "unknown-not-assessed",
        "disclosure_eligibility": "unknown-not-assessed",
        "private_cohort_bindings_disclosed": False,
        "capture_close_match": "not-assessed-pending-approved-statistic-health-and-disclosure-policy",
        "qualification": "not-assessed",
        "interpretation": "aggregate-only; identity-level cohorts remain private; numerical bounds and descriptive alignment do not constitute a registered verdict",
    }


def _compare_captures(args: argparse.Namespace) -> dict[str, Any]:
    project = _read_cross_capture_summary(Path(args.project_summary))
    outsider = _read_cross_capture_summary(Path(args.outsider_summary))
    result = _compare_capture_summaries(project, outsider, args.arm)
    out = Path(args.out)
    _new_private_dir(out)
    out = out.resolve(strict=True)
    _atomic_json(out / "cross-capture-summary.json", result)
    return {"status": result["status"], "summary": str(out / "cross-capture-summary.json"), "capture_close_match": result["capture_close_match"]}


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="mode", required=True)
    for mode in ("public", "compare"):
        q = sub.add_parser(mode)
        q.add_argument("--manifest", required=True)
        q.add_argument("--archive-dir", required=True)
        q.add_argument("--out", required=True)
        if mode == "compare":
            q.add_argument("--public-run-dir", required=True)
            q.add_argument("--sfdps-dir", required=True)
    q = sub.add_parser("compare-captures")
    q.add_argument("--project-summary", required=True)
    q.add_argument("--outsider-summary", required=True)
    q.add_argument("--arm", required=True)
    q.add_argument("--out", required=True)
    return p


def main(argv: list[str] | None = None) -> int:
    try:
        args = _parser().parse_args(argv)
    except SystemExit as exc:
        return exc.code if isinstance(exc.code, int) else 1
    try:
        if args.mode == "public":
            result = _public_run(args)
        elif args.mode == "compare":
            result = _compare_run(args)
        else:
            result = _compare_captures(args)
        print(json.dumps(result, ensure_ascii=True, sort_keys=True, separators=(",", ":")))
        return 0
    except Refused as exc:
        print(f"reproduce_p010: refused: {exc}", file=sys.stderr)
        return 2
    except BaseException as exc:
        print(f"reproduce_p010: INCOMPLETE: {type(exc).__name__}: {str(exc)[:2048]}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
