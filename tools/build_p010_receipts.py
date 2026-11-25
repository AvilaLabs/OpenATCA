#!/usr/bin/env python3
"""Build the exact P-010 binaries and bind their Cargo/guard evidence; never runs a scorer."""
from __future__ import annotations

import argparse
import fcntl
import hashlib
import importlib.util
import json
import math
import os
import selectors
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

TOOLS = Path(__file__).resolve(strict=True).parent
ROOT = TOOLS.parent
REPLAY_ENV = TOOLS / "replay_clean_env.sh"
WRAPPER_SPEC = importlib.util.spec_from_file_location("p010_receipt_wrapper", TOOLS / "reproduce_p010.py")
if WRAPPER_SPEC is None or WRAPPER_SPEC.loader is None:
    raise RuntimeError("cannot load P-010 receipt contract")
p010 = importlib.util.module_from_spec(WRAPPER_SPEC)
WRAPPER_SPEC.loader.exec_module(p010)

MAX_CARGO_METADATA = 32 * 1024 * 1024
MAX_CARGO_STDOUT = 32 * 1024 * 1024
MAX_CARGO_STDERR = 32 * 1024 * 1024
MAX_JSON_LINE = 2 * 1024 * 1024
MAX_RECEIPT = p010.MAX_BUILD_RECEIPT_BYTES
MAX_ARTIFACT_FILE = p010.LIMITS["max_output_file_bytes"]
MAX_ARTIFACT_TOTAL = 8 * 1024**3
MAX_LOCAL_ARTIFACTS = p010.MAX_LOCAL_ARTIFACTS
MAX_TARGET_FILES = p010.MAX_BUILD_TARGET_FILES
MAX_TARGET_DIRECTORIES = p010.MAX_BUILD_TARGET_DIRECTORIES
MAX_TARGET_BYTES = p010.MAX_BUILD_TARGET_BYTES
MAX_TARGET_DEPTH = p010.MAX_BUILD_TARGET_DEPTH
MAX_CHILD_SECONDS = 2700
STOP_GRACE_SECONDS = 3
MEMORY_LIMIT = 6 * 1024**3
MIN_FREE_BYTES = 64 * 1024**3
APPROVED_PATH = "/usr/bin:/bin"
TRUSTED_CARGO = Path(p010.CARGO_EXECUTABLE)
TRUSTED_RUSTC = Path(p010.RUSTC_EXECUTABLE)


class Refused(Exception):
    pass


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-finite JSON constant {value}")


def _object_no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise ValueError(f"duplicate JSON key {key!r}")
        out[key] = value
    return out


def _finite_tree(value: Any, where: str) -> None:
    if isinstance(value, float):
        if not math.isfinite(value):
            raise Refused(f"{where} contains a non-finite JSON number")
    elif isinstance(value, list):
        for item in value:
            _finite_tree(item, where)
    elif isinstance(value, dict):
        for item in value.values():
            _finite_tree(item, where)


def _json_bytes(raw: bytes, where: str) -> Any:
    try:
        value = json.loads(raw.decode("utf-8", "strict"), object_pairs_hook=_object_no_duplicates,
                           parse_constant=_reject_constant)
        _finite_tree(value, where)
    except RecursionError as exc:
        raise Refused(f"{where} exceeds the supported JSON nesting depth") from exc
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise Refused(f"{where} is not strict finite UTF-8 JSON: {exc}") from exc
    return value


def _path(raw: str, where: str) -> Path:
    path = Path(raw)
    if not path.is_absolute() or path.resolve(strict=False) != path:
        raise Refused(f"{where} must be an absolute canonical path")
    return path


def _contained(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def _hash_open_file(path: Path, cap: int) -> tuple[int, str]:
    p010._guard().check(path)
    h = hashlib.sha256()
    total = 0
    with p010._open_guarded(path) as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > cap:
            raise Refused(f"unsafe or oversized file {path}")
        while True:
            block = stream.read(min(1024 * 1024, cap + 1 - total))
            if not block:
                break
            total += len(block)
            if total > cap:
                raise Refused(f"file exceeds byte cap: {path}")
            h.update(block)
        after = os.fstat(stream.fileno())
    named = path.lstat()
    ident = lambda st: (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns)
    if ident(before) != ident(after) or ident(after) != ident(named) or total != after.st_size:
        raise Refused(f"file changed while hashing: {path}")
    return total, h.hexdigest()


def _write_new(path: Path, raw: bytes, cap: int) -> None:
    p010._guard().check(path)
    if len(raw) > cap:
        raise Refused(f"output exceeds {cap}-byte cap: {path}")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        view = memoryview(raw)
        while view:
            n = os.write(fd, view)
            if n <= 0:
                raise OSError("short write")
            view = view[n:]
        os.fsync(fd)
    finally:
        os.close(fd)
    dir_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def _sync_dir(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _require_empty_directory(path: Path, where: str) -> None:
    p010._guard().check(path)
    with os.scandir(path) as entries:
        if next(entries, None) is not None:
            raise Refused(f"{where} must remain empty until the Cargo build starts")


def _verify_lock(lock_path: Path, lock_fd: int) -> None:
    p010._guard().check(lock_path)
    if lock_fd < 0:
        raise Refused("caller lock descriptor must be nonnegative")
    try:
        inherited = os.fstat(lock_fd)
    except OSError as exc:
        raise Refused("caller lock descriptor is not open in the producer") from exc
    if not stat.S_ISREG(inherited.st_mode):
        raise Refused("caller lock descriptor must refer to a regular lock file")
    flags = os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    check_fd = os.open(lock_path, flags)
    try:
        named = os.fstat(check_fd)
        if (inherited.st_dev, inherited.st_ino) != (named.st_dev, named.st_ino):
            raise Refused("caller lock descriptor does not name the supplied lock file")
        try:
            fcntl.flock(check_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            locks_raw = p010._read_guarded(Path("/proc/locks"), 1024 * 1024).decode("ascii", "strict")
            wanted_dev = (os.major(inherited.st_dev), os.minor(inherited.st_dev), inherited.st_ino)
            ancestors = _process_ancestors()
            owner_ok = False
            for line in locks_raw.splitlines():
                fields = line.split()
                if len(fields) < 6 or fields[1] != "FLOCK" or fields[3] != "WRITE":
                    continue
                try:
                    major_text, minor_text, inode_text = fields[5].split(":", 2)
                    found_dev = (int(major_text, 16), int(minor_text, 16), int(inode_text, 10))
                    pid = int(fields[4], 10)
                except (ValueError, IndexError):
                    continue
                if found_dev == wanted_dev and pid in ancestors:
                    owner_ok = True
                    break
            if not owner_ok:
                raise Refused("exclusive lock is not held by the caller or its producer process")
            return
        fcntl.flock(check_fd, fcntl.LOCK_UN)
        raise Refused("caller has not retained the required exclusive lock")
    finally:
        os.close(check_fd)


def _process_ancestors() -> set[int]:
    ancestors: set[int] = set()
    pid = os.getpid()
    for _ in range(32):
        if pid <= 1 or pid in ancestors:
            break
        ancestors.add(pid)
        raw = p010._read_guarded(Path(f"/proc/{pid}/status"), 64 * 1024).decode("ascii", "strict")
        parent_line = next((line for line in raw.splitlines() if line.startswith("PPid:")), None)
        if parent_line is None:
            break
        try:
            pid = int(parent_line.split()[1], 10)
        except (ValueError, IndexError):
            break
    return ancestors


def _verify_envelope(paths: tuple[Path, ...]) -> None:
    try:
        cgroup_raw = p010._read_guarded(Path("/proc/self/cgroup"), 64 * 1024)
        cgroup = cgroup_raw.decode("ascii", "strict").splitlines()
        unified = [line.split("::", 1)[1] for line in cgroup if line.startswith("0::")]
        if len(unified) != 1:
            raise Refused("one cgroup-v2 memory envelope is required")
        root = Path("/sys/fs/cgroup").resolve(strict=True)
        cg = (root / unified[0].lstrip("/")).resolve(strict=True)
        cg.relative_to(root)
        memory_path = cg / "memory.max"
        swap_path = cg / "memory.swap.max"
        p010._guard().check(root)
        p010._guard().check(cg)
        memory = p010._read_guarded(memory_path, 64).decode("ascii", "strict").strip()
        swap = p010._read_guarded(swap_path, 64).decode("ascii", "strict").strip()
    except (OSError, ValueError) as exc:
        raise Refused(f"cannot verify cgroup-v2 resource limits: {exc}") from exc
    if memory == "max" or not memory.isdecimal() or int(memory) > MEMORY_LIMIT:
        raise Refused("build requires cgroup-v2 memory.max at or below 6 GiB")
    if swap != "0":
        raise Refused("build requires cgroup-v2 memory.swap.max=0")
    for path in paths:
        p010._guard().check(path)
        try:
            stats = os.statvfs(path)
        except OSError as exc:
            raise Refused(f"cannot inspect free disk at {path}: {exc}") from exc
        if stats.f_bavail * stats.f_frsize < MIN_FREE_BYTES:
            raise Refused(f"free disk below the 64 GiB floor at {path}")


def _clean_shell_argv(argv: list[str]) -> list[str]:
    # This invokes the same existing replay_clean_env function as replay_compare.sh. Values are
    # passed as argv elements, never interpolated into shell source.
    shell = (f'source "$1"; replay_clean_env; '
             f'clean_env_prefix+=("PATH={APPROVED_PATH}" "RUSTC={TRUSTED_RUSTC}" '
             '"CARGO_BUILD_JOBS=1" "CARGO_NET_OFFLINE=true"); '
             'shift; exec "${clean_env_prefix[@]}" "$@"')
    return ["/bin/bash", "-c", shell, "p010-build", str(REPLAY_ENV), *argv]


def _shell_environment() -> dict[str, str]:
    allowed = ("HOME", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR")
    return {"PATH": APPROVED_PATH, **{key: os.environ[key] for key in allowed if key in os.environ}}


def _run_bounded(argv: list[str], cwd: Path, stdout_path: Path, stderr_path: Path,
                 stdout_cap: int, stderr_cap: int, timeout_s: int,
                 disk_paths: tuple[Path, ...], lock_path: Path,
                 lock_fd: int) -> dict[str, Any]:
    p010._guard().check(REPLAY_ENV)
    p010._guard().check(cwd)
    for path in (stdout_path, stderr_path):
        p010._guard().check(path)
    out_fd = os.open(stdout_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    err_fd = -1
    proc: subprocess.Popen[bytes] | None = None
    selector: selectors.BaseSelector | None = None
    counts = {"stdout": 0, "stderr": 0}
    overflow = timed_out = False
    start = time.monotonic()
    try:
        err_fd = os.open(stderr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        proc = subprocess.Popen(_clean_shell_argv(argv), cwd=cwd, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                start_new_session=True, close_fds=True, env=_shell_environment())
        selector = selectors.DefaultSelector()
        pipes = ((proc.stdout, "stdout", out_fd, stdout_cap), (proc.stderr, "stderr", err_fd, stderr_cap))
        for pipe, label, _, _ in pipes:
            if pipe is None:
                raise Refused("build child pipe was not created")
            os.set_blocking(pipe.fileno(), False)
            selector.register(pipe, selectors.EVENT_READ, label)
        while selector.get_map() or proc.poll() is None:
            if time.monotonic() - start >= timeout_s - STOP_GRACE_SECONDS:
                timed_out = True
                break
            _verify_lock(lock_path, lock_fd)
            _verify_envelope(disk_paths)
            for key, _ in selector.select(0.5):
                block = os.read(key.fd, 65536)
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                label = key.data
                counts[label] += len(block)
                cap = stdout_cap if label == "stdout" else stderr_cap
                if counts[label] > cap:
                    overflow = True
                    break
                fd = out_fd if label == "stdout" else err_fd
                view = memoryview(block)
                while view:
                    written = os.write(fd, view)
                    if written <= 0:
                        raise OSError("short build log write")
                    view = view[written:]
            if overflow:
                break
        if timed_out or overflow:
            p010._stop_owned(proc)
        else:
            proc.wait()
    finally:
        if proc is not None and proc.poll() is None:
            p010._stop_owned(proc)
        if selector is not None:
            selector.close()
        if proc is not None:
            for pipe in (proc.stdout, proc.stderr):
                if pipe is not None and not pipe.closed:
                    pipe.close()
        for fd in (out_fd, err_fd):
            if fd >= 0:
                os.fsync(fd)
                os.close(fd)
    return {"exit_code": proc.returncode if proc is not None else None,
            "timed_out": timed_out, "capture_limit_exceeded": overflow,
            "elapsed_s": time.monotonic() - start, "log_bytes": counts}


def _toolchain_snapshot(out: Path, phase: str, disk_paths: tuple[Path, ...],
                        lock_path: Path, lock_fd: int) -> dict[str, dict[str, Any]]:
    records: dict[str, dict[str, Any]] = {}
    for name, executable, prefix in (("cargo", TRUSTED_CARGO, b"cargo 1.89.0 "),
                                     ("rustc", TRUSTED_RUSTC, b"rustc 1.89.0 ")):
        p010._guard().check(executable)
        if executable.resolve(strict=True) != executable:
            raise Refused(f"trusted {name} tool path is not canonical")
        before_size, before_sha = _hash_open_file(executable, 128 * 1024 * 1024)
        st = executable.lstat()
        if not stat.S_ISREG(st.st_mode) or not os.access(executable, os.X_OK):
            raise Refused(f"trusted {name} tool is not a regular executable")
        stdout_path = out / "logs" / f"toolchain.{phase}.{name}.stdout"
        stderr_path = out / "logs" / f"toolchain.{phase}.{name}.stderr"
        run = _run_bounded([str(executable), "--version"], ROOT, stdout_path, stderr_path,
                           1024, 1024, 10, disk_paths, lock_path, lock_fd)
        stdout = _read_capped(stdout_path, 1024)
        stderr = _read_capped(stderr_path, 1024)
        after_size, after_sha = _hash_open_file(executable, 128 * 1024 * 1024)
        if (run["exit_code"] != 0 or run["timed_out"] or run["capture_limit_exceeded"]
                or stderr or before_size != after_size or before_sha != after_sha
                or not stdout.startswith(prefix) or not stdout.endswith(b"\n") or stdout.count(b"\n") != 1):
            raise Refused(f"trusted {name} executable failed its pinned path/version/hash check")
        records[name] = {"path": str(executable), "bytes": after_size,
                         "sha256": after_sha, "version": stdout.decode("ascii", "strict").strip()}
    return records


def _read_capped(path: Path, cap: int) -> bytes:
    return p010._read_guarded(path, cap)


def _cargo_events(path: Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    total = 0
    with p010._open_guarded(path) as stream:
        while True:
            line = stream.readline(MAX_JSON_LINE + 1)
            if not line:
                break
            total += len(line)
            if len(line) > MAX_JSON_LINE or total > MAX_CARGO_STDOUT:
                raise Refused("Cargo JSON-lines output exceeds its fixed cap")
            value = _json_bytes(line, "Cargo JSON event")
            if not isinstance(value, dict):
                raise Refused("Cargo output line is not a JSON object")
            events.append(value)
            if len(events) > 200_000:
                raise Refused("Cargo emitted more events than the fixed event cap")
    return events


def _package_graph(metadata: Any, source_root: Path, package_name: str,
                   manifest: Path) -> tuple[dict[str, dict[str, Any]], set[str]]:
    if not isinstance(metadata, dict) or not isinstance(metadata.get("packages"), list):
        raise Refused("Cargo metadata packages field is missing")
    packages: dict[str, dict[str, Any]] = {}
    manifest_seen: set[Path] = set()
    for index, package in enumerate(metadata["packages"]):
        if not isinstance(package, dict):
            raise Refused(f"Cargo metadata package {index} is not an object")
        package_id = package.get("id")
        manifest_text = package.get("manifest_path")
        source = package.get("source")
        if not isinstance(package_id, str) or not package_id or len(package_id) > 4096:
            raise Refused("Cargo metadata package ID is malformed")
        if package_id in packages:
            raise Refused("Cargo metadata contains duplicate package IDs")
        if ("source" not in package or not isinstance(manifest_text, str)
                or not Path(manifest_text).is_absolute() or (source is not None and not isinstance(source, str))):
            raise Refused("Cargo metadata package source/manifest fields are malformed")
        package_manifest = Path(manifest_text)
        p010._guard().check(package_manifest)
        resolved = package_manifest.resolve(strict=True)
        if resolved != package_manifest or package_manifest.name != "Cargo.toml":
            raise Refused("Cargo metadata package manifest path is noncanonical")
        if package_manifest in manifest_seen:
            raise Refused("Cargo metadata contains a duplicate manifest path")
        manifest_seen.add(package_manifest)
        if source is None and not _contained(package_manifest, source_root):
            raise Refused("Cargo metadata contains a foreign local path package")
        if package.get("name") is not None and not isinstance(package.get("name"), str):
            raise Refused("Cargo metadata package name is malformed")
        packages[package_id] = {"raw": package, "manifest": package_manifest, "source": source}

    roots = [package_id for package_id, package in packages.items()
             if package["source"] is None and package["manifest"] == manifest and
             package["raw"].get("name") == package_name]
    if len(roots) != 1:
        raise Refused("Cargo metadata does not identify exactly one requested local package")
    resolve = metadata.get("resolve")
    if not isinstance(resolve, dict) or not isinstance(resolve.get("nodes"), list):
        raise Refused("Cargo metadata dependency resolution graph is missing")
    nodes: dict[str, dict[str, Any]] = {}
    for node in resolve["nodes"]:
        if not isinstance(node, dict) or not isinstance(node.get("id"), str) or node["id"] in nodes:
            raise Refused("Cargo metadata contains malformed or duplicate resolution nodes")
        if node["id"] not in packages or not isinstance(node.get("deps"), list):
            raise Refused("Cargo resolution node references an unknown package")
        nodes[node["id"]] = node
    reachable: set[str] = set()
    pending = list(roots)
    while pending:
        package_id = pending.pop()
        if package_id in reachable:
            continue
        node = nodes.get(package_id)
        if node is None:
            raise Refused("Cargo dependency graph omits a reachable package")
        reachable.add(package_id)
        for dep in node["deps"]:
            if not isinstance(dep, dict) or not isinstance(dep.get("pkg"), str) or dep["pkg"] not in packages:
                raise Refused("Cargo dependency graph contains a malformed package edge")
            pending.append(dep["pkg"])
    return packages, reachable


def _source_tree_oid(source_root: Path, commit: str, package_manifest: Path) -> str:
    relative = package_manifest.parent.relative_to(source_root)
    git_path = relative.as_posix() or "."
    raw = p010._git(source_root, "rev-parse", f"{commit}:{git_path}", limit=4096)
    value = raw.decode("ascii", "strict").strip()
    if not p010.HEX40.fullmatch(value):
        raise Refused("Cargo local package does not resolve to a Git tree object")
    return value


def _hash_cargo_file(raw: Any, target_dir: Path, where: str,
                     seen_files: set[Path]) -> tuple[dict[str, Any], int]:
    if not isinstance(raw, str) or not Path(raw).is_absolute():
        raise Refused(f"{where} artifact path must be absolute")
    path = Path(raw)
    p010._guard().check(path)
    if path.resolve(strict=True) != path or not _contained(path, target_dir):
        raise Refused(f"{where} artifact resolves outside its unique Cargo target")
    if path in seen_files:
        raise Refused(f"Cargo metadata repeated artifact filename {path}")
    st = path.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_size > MAX_ARTIFACT_FILE:
        raise Refused(f"Cargo artifact is not a bounded regular file: {path}")
    size, digest = _hash_open_file(path, MAX_ARTIFACT_FILE)
    seen_files.add(path)
    return {"path": str(path), "bytes": size, "sha256": digest}, size


def _measure_target(target_dir: Path) -> dict[str, int]:
    p010._guard().check(target_dir)
    root_stat = target_dir.lstat()
    if not stat.S_ISDIR(root_stat.st_mode) or target_dir.is_symlink():
        raise Refused("Cargo target root is not a plain directory")
    files = 0
    directories = 1
    total_bytes = 0
    pending: list[tuple[Path, int]] = [(target_dir, 0)]
    while pending:
        directory, depth = pending.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                path = Path(entry.path)
                p010._guard().check(path)
                st = entry.stat(follow_symlinks=False)
                if stat.S_ISDIR(st.st_mode):
                    directories += 1
                    if directories > MAX_TARGET_DIRECTORIES or depth + 1 > MAX_TARGET_DEPTH:
                        raise Refused("Cargo target directory count or depth exceeds its fixed cap")
                    pending.append((path, depth + 1))
                elif stat.S_ISREG(st.st_mode):
                    files += 1
                    total_bytes += st.st_size
                    if files > MAX_TARGET_FILES or st.st_size > MAX_ARTIFACT_FILE or total_bytes > MAX_TARGET_BYTES:
                        raise Refused("Cargo target file count or bytes exceed their fixed caps")
                else:
                    raise Refused(f"Cargo target contains a symlink or special file: {path}")
    return {"files": files, "directories": directories, "bytes": total_bytes,
            "max_files": MAX_TARGET_FILES, "max_directories": MAX_TARGET_DIRECTORIES,
            "max_bytes": MAX_TARGET_BYTES, "max_depth": MAX_TARGET_DEPTH}


def _inspect_artifacts(events: list[dict[str, Any]], packages: dict[str, dict[str, Any]],
                       reachable: set[str], source_root: Path, commit: str,
                       target_dir: Path, role: str, artifact_name: str) -> tuple[dict[str, Any], list[dict[str, Any]], dict[str, Any]]:
    target_path = {"clipper": "oa-adsb", "candidate": "oa-track-tools",
                   "candidate_scorer": "oa-track-tools", "replication_scorer": "oa-track"}[role]
    expected_manifest = source_root / "crates" / target_path / "Cargo.toml"
    selected: list[dict[str, Any]] = []
    sealed_events: list[dict[str, Any]] = []
    digest_events: list[dict[str, Any]] = []
    local_artifacts: list[dict[str, Any]] = []
    event_keys: set[tuple[str, str, tuple[str, ...]]] = set()
    seen_files: set[Path] = set()
    reported_paths: set[Path] = set()
    total_bytes = 0
    compiler_count = 0
    for event in events:
        if event.get("reason") != "compiler-artifact":
            continue
        compiler_count += 1
        package_id = event.get("package_id")
        target = event.get("target")
        manifest_text = event.get("manifest_path")
        filenames = event.get("filenames")
        if (not isinstance(package_id, str) or package_id not in packages or
                not isinstance(target, dict) or not isinstance(target.get("name"), str) or len(target["name"]) > 256 or
                not isinstance(target.get("kind"), list) or not 1 <= len(target["kind"]) <= 8 or
                any(not isinstance(kind, str) or not kind for kind in target["kind"]) or
                len(set(target["kind"])) != len(target["kind"]) or
                not isinstance(manifest_text, str) or not isinstance(filenames, list) or not 1 <= len(filenames) <= 128):
            raise Refused("Cargo compiler-artifact event has malformed identity fields")
        manifest = Path(manifest_text)
        package = packages[package_id]
        if manifest != package["manifest"]:
            raise Refused("Cargo compiler-artifact manifest does not match cargo metadata package identity")
        if event.get("fresh") is not False:
            raise Refused("Cargo reused a fresh/cached compiler artifact instead of compiling in the empty target")
        identity = (package_id, target["name"], tuple(target["kind"]))
        if identity in event_keys:
            raise Refused("Cargo emitted duplicate compiler-artifact identity")
        event_keys.add(identity)
        records: list[str] = []
        file_records: list[dict[str, Any]] = []
        for filename in filenames:
            if not isinstance(filename, str) or not Path(filename).is_absolute():
                raise Refused("Cargo emitted a nonabsolute artifact filename")
            path = Path(filename)
            p010._guard().check(path)
            if path.resolve(strict=True) != path or not _contained(path, target_dir):
                raise Refused("Cargo emitted an artifact outside this role's isolated target")
            if path in reported_paths:
                raise Refused(f"Cargo JSON metadata repeats artifact filename {path}")
            reported_paths.add(path)
            if package["source"] is None:
                record, size = _hash_cargo_file(filename, target_dir, role, seen_files)
                total_bytes += size
                if total_bytes > MAX_ARTIFACT_TOTAL:
                    raise Refused("local Cargo artifact bytes exceed the fixed receipt ceiling")
                file_records.append(record)
            else:
                st = path.lstat()
                if not stat.S_ISREG(st.st_mode) or st.st_size > MAX_ARTIFACT_FILE:
                    raise Refused("registry artifact is not a bounded regular file")
            records.append(str(path))
        if package["source"] is None:
            if package_id not in reachable:
                raise Refused("Cargo compiled a local package outside the selected package dependency graph")
            tree_oid = _source_tree_oid(source_root, commit, manifest)
            local_artifacts.append({"package_id": package_id, "manifest_path": str(manifest),
                                    "package_tree_git_oid": tree_oid, "target_name": target["name"],
                                    "kind": target["kind"], "fresh": False, "filenames": file_records})
        if manifest == expected_manifest and target["name"] == artifact_name:
            selected.append({"binary": next((str(Path(name)) for name in records
                                               if Path(name).name == artifact_name and os.access(name, os.X_OK)), None),
                             "manifest_path": str(manifest), "target_name": target["name"],
                             "fresh": False, "filenames": records})
        if package["source"] is None and manifest == source_root / "crates/oa-sealed/Cargo.toml" and target["name"] == "oa_sealed" and "lib" in target["kind"]:
            sealed_events.append({"manifest_path": str(manifest), "fresh": False, "filenames": records})
        if package["source"] is None and manifest == source_root / "crates/oa-sha256/Cargo.toml" and target["name"] == "oa_sha256" and "lib" in target["kind"]:
            digest_events.append({"manifest_path": str(manifest), "fresh": False, "filenames": records})
    if compiler_count == 0 or len(selected) != 1 or not selected[0]["binary"]:
        raise Refused(f"Cargo did not emit exactly one executable artifact for {role}/{artifact_name}")
    if len(sealed_events) != 1 or len(digest_events) != 1:
        raise Refused(f"Cargo did not emit exactly one oa-sealed and oa-sha256 library for {role}")
    if len(local_artifacts) > MAX_LOCAL_ARTIFACTS:
        raise Refused("local Cargo artifact count exceeds the fixed cap")
    return selected[0], local_artifacts, {"oa_sealed": sealed_events[0], "oa_sha256": digest_events[0]}


def _run_probe(binary: Path, expected: bytes, out_dir: Path,
               role: str, target_dir: Path, lock_path: Path,
               lock_fd: int) -> tuple[bytes, dict[str, Any]]:
    stdout_path = out_dir / f"{role}.probe.stdout"
    stderr_path = out_dir / f"{role}.probe.stderr"
    result = _run_bounded([str(binary)], ROOT, stdout_path, stderr_path, 1024, 1024, 10,
                          (out_dir, target_dir.parent), lock_path, lock_fd)
    stdout = _read_capped(stdout_path, 1024)
    stderr = _read_capped(stderr_path, 1024)
    if result["exit_code"] != 0 or result["timed_out"] or result["capture_limit_exceeded"] or stdout != expected or stderr:
        raise Refused(f"{role} compiled production/embedded guard probe failed its fixed synthetic assertions")
    return stdout, result


def _build_role(doc: dict[str, Any], role: str, out: Path,
                lock_path: Path, lock_fd: int,
                expected_toolchain: dict[str, dict[str, Any]]) -> tuple[dict[str, Any], Path, Path]:
    build = doc["builds"][role]
    source = p010._build_source(doc, role)
    p010._guard().check(Path(source["tree"]))
    source_root = Path(source["tree"]).resolve(strict=True)
    commit = source["commit"]
    target_dir = p010._canonical_build_path(build["target_dir"], f"builds.{role}.target_dir")
    p010._guard().check(target_dir)
    if target_dir.exists() or target_dir.is_symlink():
        raise Refused(f"target directory is not fresh and absent: {target_dir}")
    if not target_dir.parent.is_dir() or target_dir.parent.is_symlink():
        raise Refused(f"target directory parent must already be a plain directory: {target_dir.parent}")
    target_dir.mkdir(mode=0o700, exist_ok=False)
    toolchain_before = _toolchain_snapshot(out, f"{role}.before", (out, target_dir.parent), lock_path, lock_fd)
    if toolchain_before != expected_toolchain:
        raise Refused(f"{role} trusted toolchain differs from the initial pinned binaries")
    source_before = p010._source_state(source["tree"], commit)
    cargo_meta = out / "logs" / f"{role}.cargo-metadata.json"
    cargo_meta_stderr = out / "logs" / f"{role}.cargo-metadata.stderr"
    meta_argv = [str(TRUSTED_CARGO), "metadata", "--format-version", "1", "--locked", "--offline",
                 "--manifest-path", str(source_root / "Cargo.toml")]
    meta_run = _run_bounded(meta_argv, source_root, cargo_meta, cargo_meta_stderr,
                            MAX_CARGO_METADATA, MAX_CARGO_STDERR, 300, (out, target_dir.parent), lock_path, lock_fd)
    if meta_run["exit_code"] != 0 or meta_run["timed_out"] or meta_run["capture_limit_exceeded"]:
        raise Refused(f"{role} Cargo metadata command failed or exceeded a bound")
    metadata = _json_bytes(_read_capped(cargo_meta, MAX_CARGO_METADATA), f"{role} Cargo metadata")
    package_name, _target = {"clipper": ("oa-adsb", "oa-adsb-clip"),
                             "candidate": ("oa-track-tools", "oa-track-run"),
                             "candidate_scorer": ("oa-track-tools", "p002"),
                             "replication_scorer": ("oa-track", "p002")}[role]
    package_manifest = source_root / "crates" / {"clipper": "oa-adsb", "candidate": "oa-track-tools",
                                                  "candidate_scorer": "oa-track-tools", "replication_scorer": "oa-track"}[role] / "Cargo.toml"
    packages, reachable = _package_graph(metadata, source_root, package_name, package_manifest)
    source_after_metadata = p010._source_state(source["tree"], commit)
    if source_after_metadata != source_before:
        raise Refused(f"{role} source tree changed during Cargo metadata inspection")
    _require_empty_directory(target_dir, f"{role} isolated target")

    stdout_path = out / "logs" / f"{role}.cargo.stdout.jsonl"
    stderr_path = out / "logs" / f"{role}.cargo.stderr"
    argv = p010._build_argv(role, target_dir)
    run = _run_bounded(argv, source_root, stdout_path, stderr_path, MAX_CARGO_STDOUT,
                       MAX_CARGO_STDERR, MAX_CHILD_SECONDS, (out, target_dir.parent), lock_path, lock_fd)
    source_after_cargo = p010._source_state(source["tree"], commit)
    if source_after_cargo != source_before or run["exit_code"] != 0 or run["timed_out"] or run["capture_limit_exceeded"]:
        raise Refused(f"{role} Cargo release build failed, changed source, or exceeded a bound")
    events = _cargo_events(stdout_path)
    selected, local_artifacts, library_events = _inspect_artifacts(
        events, packages, reachable, source_root, commit, target_dir, role, p010.ARTIFACT_NAMES[role])
    binary = Path(selected["binary"])
    if binary != Path(build["binary"]).resolve(strict=False):
        raise Refused(f"{role} manifest binary path differs from the actual Cargo compiler artifact")
    binary_record, _binary_nbytes = _hash_cargo_file(str(binary), target_dir, role, set())
    if binary_record["bytes"] > MAX_ARTIFACT_FILE:
        raise Refused(f"{role} executable exceeds its artifact cap")
    selected_artifact = {"binary": str(binary), "sha256": binary_record["sha256"],
                         "cargo_artifact": {key: selected[key] for key in ("manifest_path", "target_name", "fresh", "filenames")}}
    source_list = source_root / "crates/oa-sealed/sealed_paths.txt"
    _, source_list_sha = _hash_open_file(source_list, 2 * 1024 * 1024)
    rlibs: dict[str, tuple[Path, str, dict[str, Any]]] = {}
    for library, event in library_events.items():
        candidates = [Path(name) for name in event["filenames"] if name.endswith(".rlib")]
        if len(candidates) != 1:
            raise Refused(f"{role} does not have exactly one fresh Cargo rlib for {library}")
        rlib = candidates[0]
        size, digest = _hash_open_file(rlib, MAX_ARTIFACT_FILE)
        rlibs[library] = (rlib, digest, event)
    sealed_rlib, sealed_sha, sealed_event = rlibs["oa_sealed"]
    digest_rlib, digest_sha, digest_event = rlibs["oa_sha256"]
    depinfo = target_dir / "release" / "deps" / (sealed_rlib.name.removeprefix("lib").removesuffix(".rlib") + ".d")
    depinfo_raw = _read_capped(depinfo, 2 * 1024 * 1024)
    dep_text = depinfo_raw.decode("utf-8", "strict")
    # The dep-info records the include_str! input path; CARGO_MANIFEST_DIR itself is a
    # compile-time environment value and is not reliably emitted as a dep-info entry.
    if "crates/oa-sealed/src/lib.rs" not in dep_text or "crates/oa-sealed/src/../sealed_paths.txt" not in dep_text:
        raise Refused(f"{role} actual oa-sealed dep-info does not identify the source list include")

    probe_source = target_dir / "p010-seal-guard-probe.rs"
    probe_binary = target_dir / "p010-seal-guard-probe"
    if probe_source.exists() or probe_source.is_symlink() or probe_binary.exists() or probe_binary.is_symlink():
        raise Refused(f"{role} guard probe output path already exists")
    _write_new(probe_source, p010.GUARD_PROBE_SOURCE.encode("utf-8"), 16 * 1024)
    rustc_argv = [str(TRUSTED_RUSTC), "--edition=2021", str(probe_source), "-L",
                  f"dependency={target_dir}/release/deps", "--extern", f"oa_sealed={sealed_rlib}",
                  "--extern", f"oa_sha256={digest_rlib}", "-o", str(probe_binary)]
    compile_stdout = out / "logs" / f"{role}.probe-compile.stdout"
    compile_stderr = out / "logs" / f"{role}.probe-compile.stderr"
    compile_result = _run_bounded(rustc_argv, source_root, compile_stdout, compile_stderr,
                                  1024 * 1024, 1024 * 1024, 120, (out, target_dir.parent), lock_path, lock_fd)
    if compile_result["exit_code"] != 0 or compile_result["timed_out"] or compile_result["capture_limit_exceeded"]:
        raise Refused(f"{role} probe compile failed or exceeded a bound")
    _, probe_sha = _hash_open_file(probe_binary, MAX_ARTIFACT_FILE)
    expected_probe = f"P010_GUARD_PROBE_OK v1 list_sha256={source_list_sha} paths=4 timestamps=4 opens=1\n".encode("ascii")
    probe_stdout, probe_run = _run_probe(probe_binary, expected_probe, out, role, target_dir, lock_path, lock_fd)
    target_usage = _measure_target(target_dir)
    toolchain_after = _toolchain_snapshot(out, f"{role}.after", (out, target_dir.parent), lock_path, lock_fd)
    if toolchain_after != toolchain_before:
        raise Refused(f"{role} trusted tool binaries changed across build and guard probe")
    source_after_probe = p010._source_state(source["tree"], commit)
    if source_after_probe != source_before:
        raise Refused(f"{role} source tree changed during compiled guard rehearsal")

    receipt = {
        "schema": p010.BUILD_RECEIPT_SCHEMA, "role": role, "toolchain": "1.89.0",
        "toolchain_trust_assumption": p010.TOOLCHAIN_TRUST_ASSUMPTION,
        "toolchain_identity": {"before": toolchain_before, "after": toolchain_after},
        "source_before": source_before, "source_after": source_after_probe,
        "argv": argv, "exit_code": run["exit_code"], "timed_out": run["timed_out"],
        "capture_limit_exceeded": run["capture_limit_exceeded"], "non_test": True,
        "target_dir": str(target_dir), "target_dir_was_absent_before_build": True,
        "cargo_target_dir": str(target_dir),
        "cargo_wrapper_environment": {"RUSTC_WRAPPER": None, "RUSTC_WORKSPACE_WRAPPER": None,
                                       "RUSTFLAGS": None, "CARGO_BUILD_RUSTFLAGS": None,
                                       "CARGO_ENCODED_RUSTFLAGS": "", "CARGO_BUILD_RUSTC_WRAPPER": None,
                                       "CARGO_TARGET_DIR_inherited": False,
                                       "PATH": APPROVED_PATH, "RUSTC": str(TRUSTED_RUSTC)},
        "artifacts": {p010.ARTIFACT_NAMES[role]: selected_artifact},
        "local_artifacts": local_artifacts,
        "target_usage": target_usage,
        "guard_binding": {
            "source_list": str(source_list), "source_list_sha256": source_list_sha,
            "sealed_rlib": str(sealed_rlib), "sealed_rlib_sha256": sealed_sha,
            "digest_rlib": str(digest_rlib), "digest_rlib_sha256": digest_sha,
            "oa_sealed_cargo_artifact": sealed_event,
            "oa_sha256_cargo_artifact": digest_event,
            "depinfo": str(depinfo), "depinfo_sha256": p010._sha(depinfo_raw),
            "probe_source": str(probe_source), "probe_source_sha256": p010.GUARD_PROBE_SHA256,
            "probe_binary": str(probe_binary), "probe_binary_sha256": probe_sha,
            "probe_rustc_argv": rustc_argv, "probe_exit_code": probe_run["exit_code"],
            "probe_stdout": probe_stdout.decode("ascii"),
        },
    }
    receipt_raw = (json.dumps(receipt, ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    if len(receipt_raw) > MAX_RECEIPT:
        raise Refused(f"{role} receipt exceeds its fixed {MAX_RECEIPT}-byte cap")
    metadata_path = Path(build["metadata"])
    if not _contained(metadata_path, out):
        raise Refused(f"{role} metadata path must be inside the fresh private output directory")
    _write_new(metadata_path, receipt_raw, MAX_RECEIPT)
    _, metadata_sha = _hash_open_file(metadata_path, MAX_RECEIPT)
    return {"metadata_sha256": metadata_sha, "binary_sha256": binary_record["sha256"]}, metadata_path, binary


def _prepare_out(raw: str) -> Path:
    p010._guard().check(Path(raw))
    out = _path(raw, "output directory")
    if out.exists() or out.is_symlink():
        raise Refused("output directory must be fresh")
    if not out.parent.is_dir() or out.parent.is_symlink():
        raise Refused("output parent must already be a plain directory")
    out.mkdir(mode=0o700, exist_ok=False)
    return out.resolve(strict=True)


def _record_incomplete(out: Path, manifest_sha: str, exc: Exception) -> None:
    marker = out / "INCOMPLETE.json"
    if marker.exists():
        return
    incomplete = {"schema": "p010-build-rehearsal-v1", "status": "INCOMPLETE",
                  "error": f"{type(exc).__name__}: {str(exc)[:1024]}",
                  "input_manifest_sha256": manifest_sha, "score_invocations": 0}
    _write_new(marker, (json.dumps(incomplete, ensure_ascii=True, sort_keys=True, indent=2) + "\n").encode("ascii"), 64 * 1024)


def _prepare_build_set(doc: dict[str, Any], out: Path, lock_path: Path,
                       lock_fd: int) -> tuple[dict[str, dict[str, str]], tuple[Path, ...], dict[str, dict[str, str]]]:
    build_meta_dir = out / "builds"
    logs_dir = out / "logs"
    build_meta_dir.mkdir(mode=0o700)
    logs_dir.mkdir(mode=0o700)
    source_roles = {role: p010._build_source(doc, role) for role in p010.BUILD_ROLES}
    unique_sources = {src["tree"]: src for src in source_roles.values()}
    for source in unique_sources.values():
        p010._source_state(source["tree"], source["commit"])
    p010._source_relation(doc)
    targets = [p010._canonical_build_path(doc["builds"][role]["target_dir"], f"builds.{role}.target_dir")
               for role in p010.BUILD_ROLES]
    for target in targets:
        p010._guard().check(target)
        if target.exists() or target.is_symlink():
            raise Refused(f"all exclusive target directories must be absent before any build: {target}")
    for target in targets:
        parent = target.parent
        p010._guard().check(parent)
        parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if parent.is_symlink() or parent.resolve(strict=True) != parent:
            raise Refused(f"target parent is not a canonical plain directory: {parent}")
    metadata_paths = [Path(doc["builds"][role]["metadata"]) for role in p010.BUILD_ROLES]
    for role, path in zip(p010.BUILD_ROLES, metadata_paths):
        p010._guard().check(path)
        if not _contained(path, out) or path.exists() or path.is_symlink():
            raise Refused(f"{role} receipt path must be fresh and inside the new output directory")
    disk_paths = (ROOT, out, *[path.parent for path in targets])
    _verify_envelope(disk_paths)
    run_before = {key: p010._source_state(src["tree"], src["commit"]) for key, src in unique_sources.items()}
    _verify_lock(lock_path, lock_fd)
    return unique_sources, disk_paths, run_before


def _run(manifest_path: Path, output_path: str, lock_path: Path, lock_fd: int) -> Path:
    p010._guard().check(manifest_path)
    p010._guard().check(lock_path)
    doc, manifest_sha = p010._validate_manifest(manifest_path)
    _verify_lock(lock_path, lock_fd)
    out = _prepare_out(output_path)
    _write_new(out / "STARTED.json", (json.dumps({"schema": "p010-build-rehearsal-v1",
                                                   "status": "BUILDING_NO_SCORING",
                                                   "input_manifest_sha256": manifest_sha,
                                                   "score_invocations": 0},
                                                  sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"),
               64 * 1024)
    try:
        unique_sources, disk_paths, run_before = _prepare_build_set(doc, out, lock_path, lock_fd)
    except Exception as exc:
        _record_incomplete(out, manifest_sha, exc)
        raise
    # All role targets were checked absent together above, under the caller-held lock.
    build_hashes: dict[str, dict[str, str]] = {}
    try:
        toolchain_initial = _toolchain_snapshot(out, "initial", disk_paths, lock_path, lock_fd)
        for role in p010.BUILD_ROLES:
            _verify_lock(lock_path, lock_fd)
            _verify_envelope(disk_paths)
            receipt_hashes, _, _ = _build_role(doc, role, out, lock_path, lock_fd, toolchain_initial)
            build_hashes[role] = receipt_hashes
            for key, src in unique_sources.items():
                if p010._source_state(src["tree"], src["commit"]) != run_before[key]:
                    raise Refused(f"source checkout changed during the complete build set: {key}")
        bound = json.loads(json.dumps(doc))
        for role, hashes in build_hashes.items():
            bound["builds"][role]["metadata_sha256"] = hashes["metadata_sha256"]
            bound["builds"][role]["binary_sha256"] = hashes["binary_sha256"]
        bound_path = out / "manifest.bound.json"
        bound_raw = (json.dumps(bound, ensure_ascii=True, sort_keys=True, indent=2) + "\n").encode("ascii")
        _write_new(bound_path, bound_raw, p010.LIMITS["max_manifest_bytes"])
        validated, bound_sha = p010._validate_manifest(bound_path)
        binaries = p010._preflight(validated, "compare")
        if set(binaries) != set(p010.BUILD_ROLES):
            raise Refused("receipt rehearsal did not accept every exact P-010 build role")
        toolchain_final = _toolchain_snapshot(out, "final", disk_paths, lock_path, lock_fd)
        if toolchain_final != toolchain_initial:
            raise Refused("trusted toolchain changed during the complete build and probe set")
        for key, src in unique_sources.items():
            if p010._source_state(src["tree"], src["commit"]) != run_before[key]:
                raise Refused(f"source checkout changed during receipt rehearsal: {key}")
        complete = {"schema": "p010-build-rehearsal-v1", "status": "BUILD_AND_GUARD_PROBES_COMPLETE_NO_SCORING",
                    "input_manifest_sha256": manifest_sha, "bound_manifest": str(bound_path),
                    "bound_manifest_sha256": bound_sha, "roles": list(p010.BUILD_ROLES),
                    "producer_sha256": _hash_open_file(Path(__file__), MAX_RECEIPT)[1],
                    "replay_clean_env_sha256": _hash_open_file(REPLAY_ENV, 64 * 1024)[1],
                    "score_invocations": 0}
        complete_raw = (json.dumps(complete, ensure_ascii=True, sort_keys=True, indent=2) + "\n").encode("ascii")
        _write_new(out / "COMPLETE.json", complete_raw, 64 * 1024)
        _sync_dir(out)
        return out
    except Exception as exc:
        _record_incomplete(out, manifest_sha, exc)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--lock-file", type=Path, required=True)
    parser.add_argument("--lock-fd", type=int, required=True)
    args = parser.parse_args(argv)
    try:
        out = _run(args.manifest, args.out, args.lock_file, args.lock_fd)
    except (Refused, p010.Refused, OSError, ValueError) as exc:
        print(f"build_p010_receipts: refused/incomplete: {exc}", file=sys.stderr)
        return 2
    print(f"build_p010_receipts: complete no-score rehearsal at {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
