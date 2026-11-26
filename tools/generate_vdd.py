#!/usr/bin/env python3
"""Generate a bounded version description from one clean OpenATCA commit."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import selectors
import time
import subprocess
import sys


# The VDD reads only small metadata registers; oversized sources fail before decoding.
MAX_COMMAND_BYTES = 8 * 1024 * 1024  # Bounds git history/status bytes before parsing.
MAX_RELEASE_TAGS = 2_000  # A release repository with more tags requires a deliberate limit review.
MAX_COMMITS = 5_000  # Bounds history entries retained and grouped in one VDD.
MAX_REQUIREMENTS = 20_000  # Bounds parsed trace counts; current register is far smaller.
MAX_PUBLIC_LIMIT_ROWS = 512  # Bounds all status/capability/unknown table rows together.
MAX_PUBLIC_SOURCE_LINES = 16_384  # Bounds line splitting across the three small public registers.
# CI builds executable products only: at most 512 files, each <=512 MiB, aggregate <=4 GiB.
MAX_BUILD_FILES = 512
MAX_ONE_BUILD_FILE_BYTES = 512 * 1024 * 1024
MAX_TOTAL_BUILD_BYTES = 4 * 1024 * 1024 * 1024
MAX_OUTPUT_BYTES = 4 * 1024 * 1024  # Includes the whole rendered VDD and each generated section.
MAX_PATH_BYTES = 4_096  # Linux path-length ceiling, applied to accepted CLI paths/names.
MAX_TRACE_BYTES = 2 * 1024 * 1024  # Trace report is a compact summary, not copied wholesale.
MAX_HAZARD_BYTES = 2 * 1024 * 1024  # Matches the safety-table generator's hazard input ceiling.
MAX_PROVENANCE_BYTES = 32 * 1024 * 1024  # Room for the bounded toolchain component inventory.
MAX_SBOM_BYTES = 16 * 1024 * 1024  # Matches the supply-chain generator's bounded Cargo.lock input.
MAX_TEXT_SOURCE_BYTES = 2 * 1024 * 1024  # Public status registers only; each is independently capped.
VERSION_RE = re.compile(r"v([0-9]+\.[0-9]+\.[0-9]+)\Z")
REQ_RE = re.compile(r"\bOA-[A-Z0-9]+-[0-9]{3}\b")
LOCAL_PATH_RE = re.compile(r"/(?:home|tmp)/[^\s`]+")
PRIVATE_SEGMENT_PATH_RE = re.compile(r"(?<![A-Za-z0-9_.-])(?:[A-Za-z0-9_.-]+/)*private(?:/[A-Za-z0-9_.-]+)+")


class VddError(Exception):
    pass


class BoundedText:
    """Append bounded UTF-8 text without constructing an oversized report first."""

    def __init__(self, limit: int):
        self.limit = limit
        self.size = 0
        self.parts: list[str] = []

    def append(self, value: str) -> None:
        size = len(value.encode("utf-8"))
        if self.size + size > self.limit:
            raise VddError(f"rendered text exceeds {self.limit}-byte limit")
        self.parts.append(value)
        self.size += size

    def render(self) -> str:
        return "".join(self.parts)


def _run(root: Path, argv: list[str], *, cap: int = MAX_COMMAND_BYTES) -> bytes:
    """Run a fixed git command with output and wall-time caps."""
    try:
        process = subprocess.Popen(
            argv, cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        raise VddError(f"cannot start {argv[0]}: {type(exc).__name__}") from exc
    if process.stdout is None or process.stderr is None:
        process.kill()
        process.wait()
        raise VddError("cannot capture bounded command output")
    output = bytearray()
    diagnostics = bytearray()
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ, (output, cap, "output"))
    selector.register(process.stderr, selectors.EVENT_READ, (diagnostics, MAX_COMMAND_BYTES, "diagnostics"))
    deadline = time.monotonic() + 30
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise VddError(f"command exceeded 30-second limit: {argv[0]}")
            ready = selector.select(remaining)
            if not ready:
                raise VddError(f"command exceeded 30-second limit: {argv[0]}")
            for key, _ in ready:
                buffer, limit, label = key.data
                chunk = os.read(key.fileobj.fileno(), min(65_536, limit + 1 - len(buffer)))
                if not chunk:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                buffer.extend(chunk)
                if len(buffer) > limit:
                    raise VddError(f"command {label} exceeds {limit}-byte limit: {argv[0]}")
    except BaseException:
        process.kill()
        process.wait()
        raise
    finally:
        selector.close()
        for pipe in (process.stdout, process.stderr):
            if pipe is not None and not pipe.closed:
                pipe.close()
    try:
        code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
    except subprocess.TimeoutExpired as exc:
        process.kill()
        process.wait()
        raise VddError(f"command exceeded 30-second limit: {argv[0]}") from exc
    if code:
        detail = diagnostics.decode("utf-8", "replace")[:512].replace("\n", " ")
        raise VddError(f"command refused ({argv[0]}, exit {code}): {detail}")
    return bytes(output)


def _read(path: Path, cap: int) -> bytes:
    try:
        if path.is_symlink():
            raise VddError(f"refusing symlink input: {path.name}")
        before = path.stat()
        if not stat.S_ISREG(before.st_mode) or before.st_size > cap:
            raise VddError(f"input is not a regular file within {cap}-byte limit: {path.name}")
        with path.open("rb") as stream:
            data = stream.read(cap + 1)
        after = path.stat()
    except OSError as exc:
        raise VddError(f"cannot read {path.as_posix()}: {type(exc).__name__}") from exc
    if (len(data) > cap or len(data) != before.st_size or
            (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) !=
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)):
        raise VddError(f"{path.as_posix()} exceeds {cap}-byte limit")
    return data


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _git_text(root: Path, *args: str, cap: int = MAX_COMMAND_BYTES) -> str:
    return _run(root, ["git", *args], cap=cap).decode("utf-8", "strict").strip()


def _clean_source(root: Path) -> None:
    status = _run(root, ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all"], cap=MAX_COMMAND_BYTES)
    if status:
        raise VddError("refusing a dirty source tree; VDD must describe a clean commit")


def _tag_info(root: Path, mode: str, tag: str | None, date: str | None) -> tuple[str, str, str, str]:
    commit = _git_text(root, "rev-parse", "HEAD", cap=256)
    if mode == "candidate":
        if tag or date:
            raise VddError("candidate mode does not accept release tag/date")
        return "unreleased candidate", "not released", commit, "none"
    current_match = VERSION_RE.fullmatch(tag or "")
    if current_match is None:
        raise VddError("release mode requires a vMAJOR.MINOR.PATCH tag")
    tagged = _git_text(root, "rev-parse", f"{tag}^{{commit}}", cap=256)
    if tagged != commit:
        raise VddError("release tag must point exactly at HEAD")
    release_date = date or _git_text(root, "show", "-s", "--format=%cs", tagged, cap=256)
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", release_date):
        raise VddError("release date must be YYYY-MM-DD")
    return current_match.group(1), release_date, commit, tag


def _previous_tag(root: Path, current: str, supplied: str | None) -> str | None:
    if supplied:
        if not VERSION_RE.fullmatch(supplied):
            raise VddError("previous tag must be a vMAJOR.MINOR.PATCH tag")
        if supplied == current:
            raise VddError("previous tag must differ from the current release tag")
        supplied_version = tuple(map(int, VERSION_RE.fullmatch(supplied).group(1).split(".")))
        current_match = VERSION_RE.fullmatch(current)
        if current_match and supplied_version >= tuple(map(int, current_match.group(1).split("."))):
            raise VddError("previous tag must have a lower semantic version than the current tag")
        _git_text(root, "merge-base", "--is-ancestor", supplied, current, cap=256)
        return supplied
    text = _git_text(root, "tag", "--merged", "HEAD", "--list", "v*", "--sort=-version:refname", cap=256 * 1024)
    tags = text.splitlines()
    if len(tags) > MAX_RELEASE_TAGS:
        raise VddError("tag count exceeds fixed limit")
    current_match = VERSION_RE.fullmatch(current)
    current_version = tuple(map(int, current_match.group(1).split("."))) if current_match else None
    for candidate in tags:
        match = VERSION_RE.fullmatch(candidate)
        if match and (current_version is None or tuple(map(int, match.group(1).split("."))) < current_version):
            return candidate
    return None


def _changes(root: Path, baseline: str | None, commit: str) -> str:
    if baseline is None:
        raw = _run(root, ["git", "log", "-z", "--reverse", "--format=%H%x00%B", "--max-count", str(MAX_COMMITS + 1), commit], cap=MAX_COMMAND_BYTES)
        heading = "Initial release candidate; no earlier version tag is reachable."
    else:
        raw = _run(root, ["git", "log", "-z", "--reverse", "--format=%H%x00%B", "--max-count", str(MAX_COMMITS + 1), f"{baseline}..{commit}"], cap=MAX_COMMAND_BYTES)
        heading = f"Changes since `{baseline}`."
    fields = raw.decode("utf-8", "strict").split("\0")
    if fields and fields[-1] == "":
        fields.pop()
    if len(fields) % 2 != 0 or len(fields) // 2 > MAX_COMMITS:
        raise VddError(f"change history exceeds {MAX_COMMITS}-commit limit")
    grouped: dict[str, list[str]] = {}
    grouped_bytes = 0
    for index in range(0, len(fields), 2):
        commit_hash, body = fields[index], fields[index + 1]
        if not re.fullmatch(r"[0-9a-f]{40,64}", commit_hash):
            raise VddError("git change record is malformed")
        subject = body.splitlines()[0] if body.splitlines() else "(empty commit message)"
        subject = LOCAL_PATH_RE.sub("[local path omitted]", subject)
        subject = PRIVATE_SEGMENT_PATH_RE.sub("[private path omitted]", subject)
        ids = sorted(set(REQ_RE.findall(body)))
        keys = ids or ["Unassigned requirement ID"]
        entry = f"`{commit_hash[:12]}` {subject[:240]}"
        for key in keys:
            grouped_bytes += len(key.encode("utf-8")) + len(entry.encode("utf-8")) + 16
            if grouped_bytes > MAX_OUTPUT_BYTES:
                raise VddError(f"change report exceeds {MAX_OUTPUT_BYTES}-byte work limit")
            grouped.setdefault(key, []).append(entry)
    lines = BoundedText(MAX_OUTPUT_BYTES)
    lines.append(heading)
    for key in sorted(grouped):
        lines.append(f"\n**{key}**")
        for item in grouped[key]:
            lines.append(f"\n- {item}")
    lines.append("\n")
    return lines.render()


def _versions(root: Path) -> str:
    cargo = _read(root / "Cargo.toml", 128 * 1024).decode("utf-8", "strict")
    pyproject = _read(root / "python/pyproject.toml", 64 * 1024).decode("utf-8", "strict")
    rust = _read(root / "rust-toolchain.toml", 8 * 1024).decode("utf-8", "strict")
    workspace = re.search(r"(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*\"([^\"]+)\"", cargo)
    python = re.search(r"(?ms)^\[project\]\s*.*?^version\s*=\s*\"([^\"]+)\"", pyproject)
    toolchain = re.search(r"(?m)^channel\s*=\s*\"([^\"]+)\"", rust)
    if not (workspace and python and toolchain):
        raise VddError("workspace, Python, or Rust toolchain version is missing")
    adaptation = _read(root / "tools/oa-adapt/src/manifest.rs", 256 * 1024).decode("utf-8", "strict")
    snapshot = _read(root / "crates/oa-airspace/src/core/snapshot.rs", 256 * 1024).decode("utf-8", "strict")
    manifest_versions = sorted(set(re.findall(r"const (?:LEGACY|ISSUED|CA_PRESET)_MANIFEST_VERSION: u16 = ([0-9]+);", adaptation)), key=int)
    snapshot_version = re.search(r"pub const FORMAT_VERSION: u32 = ([0-9]+);", snapshot)
    if not manifest_versions or not snapshot_version:
        raise VddError("adaptation format version source is missing or unrecognized")
    return (f"- Rust workspace package version: `{workspace.group(1)}`.\n"
            f"- Python package version: `{python.group(1)}`.\n"
            f"- Rust toolchain: `{toolchain.group(1)}`.\n"
            f"- Snapshot format: `{snapshot_version.group(1)}`; adaptation manifest versions: "
            f"`{', '.join(manifest_versions)}`.\n")


def _trace_summary(path: Path) -> tuple[str, str, list[str]]:
    raw = _read(path, MAX_TRACE_BYTES)
    text = raw.decode("utf-8", "strict")
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        raise VddError("trace report is empty")
    summary = next((line for line in lines if line.startswith("Traceability (every requirement linked")), None)
    if summary is None:
        raise VddError("trace report lacks the oa-trace summary line")
    covered = next((line for line in lines if line.startswith("Requirements with tests (")), "not available")
    uncovered = next((line for line in lines if line.startswith("Requirements WITHOUT tests (")), "not available")
    counts = re.search(r"Requirements with tests \(([0-9]+)\)", covered)
    missing = re.search(r"Requirements WITHOUT tests \(([0-9]+)\)", uncovered)
    if counts is None or missing is None or int(counts.group(1)) + int(missing.group(1)) > MAX_REQUIREMENTS:
        raise VddError("trace requirement counts are missing or exceed fixed limit")
    marker = lines.index(uncovered)
    missing_ids = []
    for line in lines[marker + 1 :]:
        match = re.fullmatch(r"(OA-[A-Z0-9]+-[0-9]{3})", line)
        if match:
            missing_ids.append(match.group(1))
        elif missing_ids or line.startswith("Tags naming"):
            break
    if len(missing_ids) != int(missing.group(1)):
        raise VddError("trace uncovered requirement list does not reconcile with its count")
    detail = ("Uncovered requirement IDs: " + ", ".join(missing_ids) + "."
              if missing_ids else "Uncovered requirement IDs: none.")
    return f"{summary}; {covered}; {uncovered}; {detail}", _sha(raw), missing_ids


def _public_limits(root: Path) -> str:
    # Only the public status/capability/unknown registers feed this section; private acceptance
    # material is deliberately outside the generator's input set.
    entries = BoundedText(MAX_OUTPUT_BYTES - 512)
    row_count = 0
    partial_count = 0
    for rel in ("docs/STATUS.md", "docs/CAPABILITIES.md", "docs/UNKNOWNS.md"):
        raw = _read(root / rel, MAX_TEXT_SOURCE_BYTES)
        text = raw.decode("utf-8", "strict")
        if text.count("\n") + 1 > MAX_PUBLIC_SOURCE_LINES:
            raise VddError(f"{rel} line count exceeds public-source scan limit")
        entries.append(f"### `{rel}` (SHA-256 `{_sha(raw)}`)\n")
        for line in text.splitlines():
            if line.startswith("|") and not re.fullmatch(r"\|[\s:|-]+\|", line):
                row_count += 1
                if row_count > MAX_PUBLIC_LIMIT_ROWS:
                    raise VddError("public status/limits rows exceed fixed limit")
                if re.search(r"(?i)partial|unmet|not met|remain(?:s|ing)? open|still open|unknown", line):
                    partial_count += 1
                entries.append(f"- {line}\n")
        entries.append(f"- Source: `{rel}`; see full public file at the release commit for narrative limits.\n")
    status = (f"Public table rows: {row_count}; rows mentioning a partial, unmet, open or unknown state: {partial_count}.\n"
              if row_count else "Public status source contains no structured table rows; no partial-state rows were available.\n")
    return status + entries.render()


def _hazard_digest(root: Path) -> tuple[str, str]:
    raw = _read(root / "docs/safety/hazard-log.json", MAX_HAZARD_BYTES)
    commit = _git_text(root, "log", "-1", "--format=%H", "--", "docs/safety/hazard-log.json", cap=256)
    return commit, _sha(raw)


def _build_outputs(build_dir: Path | None) -> tuple[str, list[tuple[str, str, int]]]:
    if build_dir is None:
        return "No product build-output directory was supplied; output digests are unavailable.", []
    if build_dir.is_symlink() or not build_dir.is_dir():
        raise VddError("build output must be an existing non-symlink directory")
    entries = []
    try:
        with os.scandir(build_dir) as scan:
            for entry in scan:
                if len(entries) >= MAX_BUILD_FILES:
                    raise VddError(f"build output directory entries exceed {MAX_BUILD_FILES}-entry limit")
                entries.append(Path(entry.path))
    except OSError as exc:
        raise VddError(f"cannot enumerate build outputs: {type(exc).__name__}") from exc
    entries.sort()
    files = []
    for path in entries:
        if path.is_symlink():
            raise VddError(f"build output contains a symlink: {path.name}")
        if path.is_file() and path.stat().st_mode & 0o111:
            files.append(path)
    if len(files) > MAX_BUILD_FILES:
        raise VddError(f"build output count exceeds {MAX_BUILD_FILES}-file limit")
    results = []
    total = 0
    for path in files:
        name = path.name
        if len(os.fsencode(name)) > MAX_PATH_BYTES:
            raise VddError("build output filename exceeds path limit")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        try:
            descriptor = os.open(path, flags)
        except OSError as exc:
            raise VddError(f"cannot open build output {name}: {type(exc).__name__}") from exc
        with os.fdopen(descriptor, "rb") as stream:
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode):
                raise VddError(f"build output {name} is not a regular file")
            size = before.st_size
            if size > MAX_ONE_BUILD_FILE_BYTES:
                raise VddError(f"build output {name} exceeds per-file limit")
            total += size
            if total > MAX_TOTAL_BUILD_BYTES:
                raise VddError("build outputs exceed aggregate byte limit")
            digest = hashlib.sha256()
            seen = 0
            while True:
                block = stream.read(65_536)
                if not block:
                    break
                seen += len(block)
                if seen > MAX_ONE_BUILD_FILE_BYTES:
                    raise VddError(f"build output {name} grew past limit")
                digest.update(block)
            after = os.fstat(stream.fileno())
        if seen != size or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
                after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            raise VddError(f"build output {name} changed while hashing")
        results.append((name, digest.hexdigest(), seen))
    rendered = BoundedText(MAX_OUTPUT_BYTES)
    for name, digest, size in results:
        rendered.append(f"- Executable `{name}` — {size} bytes, SHA-256 `{digest}`\n")
    if not results:
        rendered.append("The supplied build output directory contains no executable files.\n")
    return rendered.render(), results


def _build_environment(provenance_path: Path | None) -> str:
    if provenance_path is None:
        return "Build host/provenance record was not supplied."
    raw = _read(provenance_path, MAX_PROVENANCE_BYTES)
    sbom_path = provenance_path.with_name("sbom.spdx.json")
    sbom = _read(sbom_path, MAX_SBOM_BYTES)
    try:
        data = json.loads(raw)
        rust = data["toolchain"]["rustc_fields"]
        cargo = data["toolchain"]["cargo_fields"]
        host = data["host"]
        machine = host["kernel"]["machine"]
        pretty = host["os_release"]["pretty_name"]
    except (json.JSONDecodeError, KeyError, TypeError) as exc:
        raise VddError("toolchain provenance has an unsupported schema") from exc
    if (not isinstance(rust, dict) or not isinstance(cargo, dict)
            or not isinstance(machine, str) or not isinstance(pretty, str)):
        raise VddError("toolchain provenance fields are malformed")
    return (f"Build host: `{pretty}`, kernel machine `{machine}`.\n"
            f"Cargo: `{cargo.get('release', 'unknown')}`; rustc: `{rust.get('release', 'unknown')}`; "
            f"Python: `{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}`.\n"
            f"Provenance record SHA-256 `{_sha(raw)}`.\n"
            f"SBOM SHA-256: `{_sha(sbom)}`.\n"
            "The source record identifies host/toolchain inputs and discloses that it is not a witnessed build attestation.\n")


def generate(root: Path, output: Path, *, mode: str, tag: str | None,
             release_date: str | None, previous_tag: str | None,
             trace_file: Path, build_dir: Path | None,
             provenance_file: Path | None = None) -> bytes:
    root = root.resolve(strict=True)
    resolved_output = output.resolve(strict=False)
    if resolved_output == root or root in resolved_output.parents:
        raise VddError("VDD output must be outside the source checkout")
    _clean_source(root)
    version, date, commit, tag_label = _tag_info(root, mode, tag, release_date)
    baseline = _previous_tag(root, tag or "candidate", previous_tag)
    if baseline:
        _git_text(root, "merge-base", "--is-ancestor", baseline, commit, cap=256)
    changes = _changes(root, baseline, commit)
    versions = _versions(root)
    trace_summary, trace_digest, uncovered = _trace_summary(trace_file)
    hazard_commit, hazard_digest = _hazard_digest(root)
    build_report, _build_rows = _build_outputs(build_dir)
    build_environment = _build_environment(provenance_file)
    public_limits = _public_limits(root)
    lock = _read(root / "Cargo.lock", 8 * 1024 * 1024)
    lock_digest = _sha(lock)
    build_instructions = "`cargo +1.89.0 build --workspace --release --locked`; the lockfile SHA-256 is " + f"`{lock_digest}`."
    evidence_artifacts = []
    for rel in ("docs/safety/generated/pha.csv", "docs/safety/generated/osa.csv", "docs/safety/generated/ssar.csv"):
        raw = _read(root / rel, MAX_OUTPUT_BYTES)
        evidence_artifacts.append(f"- `{rel}` — SHA-256 `{_sha(raw)}`")
    sections = [
        f"## 1. Release identity\n\n- Title: OpenATCA\n- Version: `{version}`\n- Release tag: `{tag_label}`\n- Date: `{date}`\n- Commit: `{commit}`\n- Status: `{mode}`\n",
        f"## 2. Configuration items and versions\n\n{versions}\n- Source: this repository commit.\n- Adaptation is separately versioned data; this VDD does not identify a facility-specific issued adaptation.\n",
        f"## 3. Changes since the prior baseline\n\n{changes}",
        f"## 4. Requirements trace\n\n- `oa-trace`: {trace_summary}\n- Report attachment: `requirements-trace.txt`; SHA-256 `{trace_digest}`. The trace joins source tags; it does not itself run tests or proofs.\n",
        f"## 5. Hazard record\n\n- Hazard-log commit: `{hazard_commit}`\n- Hazard-log SHA-256: `{hazard_digest}`\n- Generated safety-table artifacts:\n" + "\n".join(evidence_artifacts) + "\n",
        f"## 6. Known problems and limits\n\n{public_limits}\nUncovered requirements: {', '.join(uncovered) if uncovered else 'none in the current oa-trace report'}.\n",
        f"## 7. Build environment and output digests\n\n- Build instruction: {build_instructions}\n- {build_environment}\n- Product output digests:\n{build_report}\n- This document is generated evidence; it is not a signed build attestation.\n",
        "## 8. Installation and adaptation compatibility\n\n- Build from the release commit with the pinned toolchain and committed lockfile.\n- The release workflow records executable outputs; it does not install or activate software on processors.\n- Adaptation snapshot and manifest formats are listed in section 2; verify the target processor's accepted format before installation.\n- Facility-specific adaptation compatibility and approval must be checked against the issued package and its own recorded identity.\n",
    ]
    document = BoundedText(MAX_OUTPUT_BYTES)
    document.append("# OpenATCA Version Description Document\n\n")
    for section in sections:
        document.append(section)
        document.append("\n")
    content = document.render().encode("utf-8")
    if len(os.fsencode(output)) > MAX_PATH_BYTES:
        raise VddError("output path exceeds path limit")
    return content


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--mode", choices=("candidate", "release"), required=True)
    parser.add_argument("--tag")
    parser.add_argument("--release-date")
    parser.add_argument("--previous-tag")
    parser.add_argument("--trace-file", type=Path, required=True)
    parser.add_argument("--build-dir", type=Path)
    parser.add_argument("--provenance-file", type=Path)
    args = parser.parse_args(argv)
    try:
        for path in (args.repo, args.out, args.trace_file, args.build_dir, args.provenance_file):
            if path is not None and len(os.fsencode(path)) > MAX_PATH_BYTES:
                raise VddError("an input or output path exceeds the fixed path limit")
        content = generate(args.repo, args.out, mode=args.mode, tag=args.tag,
                           release_date=args.release_date, previous_tag=args.previous_tag,
                           trace_file=args.trace_file, build_dir=args.build_dir,
                           provenance_file=args.provenance_file)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        if args.out.exists() or args.out.is_symlink():
            raise VddError("output file must be new")
        with args.out.open("xb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
    except (OSError, UnicodeError, VddError, subprocess.TimeoutExpired) as exc:
        print(f"VDD refused: {exc}", file=sys.stderr)
        return 2
    print(f"VDD {args.out} bytes={len(content)} sha256={_sha(content)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
