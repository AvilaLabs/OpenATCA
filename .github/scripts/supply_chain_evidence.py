#!/usr/bin/env python3
"""Emit a Cargo-lock SPDX SBOM and local Rust/host build provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path


MAX_LOCK_BYTES = 16 * 1024 * 1024
MAX_MANIFEST_BYTES = 2 * 1024 * 1024
MAX_MANIFEST_FILES = 50_000
MAX_COMPONENT_BYTES = 8 * 1024**3
MAX_METADATA_BYTES = 256 * 1024
TOOL_COMMANDS = ("rustc", "cargo", "rustdoc", "rustfmt", "clippy-driver")
SAFE_COMPONENT = re.compile(r"[A-Za-z0-9_.+-]{1,180}\Z")


class EvidenceError(ValueError):
    """An input could not be safely or completely represented."""


def _read_regular(path: Path, limit: int) -> bytes:
    if path.is_symlink():
        raise EvidenceError(f"refusing symlink input: {path}")
    before = path.stat()
    if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
        raise EvidenceError(f"input is not a regular file within {limit} bytes: {path}")
    with path.open("rb") as stream:
        payload = stream.read(limit + 1)
    after = path.stat()
    if (len(payload) != before.st_size or len(payload) > limit or
            (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) !=
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
        raise EvidenceError(f"input changed while being read: {path}")
    return payload


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _member_manifests(root: Path, manifest: dict[str, object]) -> dict[tuple[str, str], dict[str, object]]:
    workspace = manifest.get("workspace")
    package_defaults = workspace.get("package", {}) if isinstance(workspace, dict) else {}
    members = workspace.get("members") if isinstance(workspace, dict) else None
    if not isinstance(members, list) or not members:
        raise EvidenceError("workspace.members must be an explicit non-empty list")
    result: dict[tuple[str, str], dict[str, object]] = {}
    for member in members:
        if not isinstance(member, str) or any(part in ("", ".", "..") for part in member.split("/")):
            raise EvidenceError("workspace member path is not a clean relative path")
        if any(char in member for char in "*?[]{}"):
            raise EvidenceError("workspace member globs are unsupported; expand them explicitly")
        candidate_path = root / member / "Cargo.toml"
        if candidate_path.is_symlink():
            raise EvidenceError(f"workspace member manifest is a symlink: {member}")
        member_path = candidate_path.resolve(strict=True)
        try:
            member_path.relative_to(root)
        except ValueError as exc:
            raise EvidenceError("workspace member escapes the repository") from exc
        raw = tomllib.loads(_read_regular(member_path, MAX_MANIFEST_BYTES).decode("utf-8"))
        package = raw.get("package")
        if not isinstance(package, dict):
            raise EvidenceError(f"workspace member has no package table: {member}")
        name = package.get("name")
        version = package.get("version")
        license_value = package.get("license")
        if version == {"workspace": True}:
            version = package_defaults.get("version")
        if license_value == {"workspace": True}:
            license_value = package_defaults.get("license")
        if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
            raise EvidenceError(f"workspace package name/version is missing: {member}")
        identity = (name, version)
        if identity in result:
            raise EvidenceError(f"duplicate workspace package identity: {name} {version}")
        result[identity] = {"manifest_path": member + "/Cargo.toml",
                            "license": license_value if isinstance(license_value, str) else None}
    return result


def _package_key(package: dict[str, object]) -> tuple[str, str, str | None]:
    name, version, source = package.get("name"), package.get("version"), package.get("source")
    if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
        raise EvidenceError("Cargo.lock package has invalid name/version")
    if source is not None and not isinstance(source, str):
        raise EvidenceError(f"Cargo.lock source is not text for {name}")
    return name, version, source


def _dependency_identity(value: object, packages: list[dict[str, object]]) -> tuple[str, str, str | None]:
    if not isinstance(value, str) or not value:
        raise EvidenceError("Cargo.lock dependency entry is malformed")
    name, separator, tail = value.partition(" ")
    if not separator:
        candidates = [item for item in packages if _package_key(item)[0] == name]
    else:
        source = None
        version = tail
        if tail.endswith(")") and " (" in tail:
            version, source = tail.rsplit(" (", 1)
            source = source[:-1]
        candidates = [item for item in packages
                      if _package_key(item) == (name, version, source)]
    identities = {_package_key(item) for item in candidates}
    if len(identities) != 1:
        raise EvidenceError(f"Cargo.lock dependency cannot be resolved unambiguously: {value}")
    return next(iter(identities))


def make_spdx(root: Path, generated_at: str | None = None) -> tuple[dict[str, object], int]:
    root = root.resolve(strict=True)
    lock_bytes = _read_regular(root / "Cargo.lock", MAX_LOCK_BYTES)
    lock = tomllib.loads(lock_bytes.decode("utf-8"))
    workspace_bytes = _read_regular(root / "Cargo.toml", MAX_MANIFEST_BYTES)
    workspace_manifest = tomllib.loads(workspace_bytes.decode("utf-8"))
    workspace_members = _member_manifests(root, workspace_manifest)
    locked = lock.get("package")
    if not isinstance(locked, list) or not locked:
        raise EvidenceError("Cargo.lock has no package records")
    packages = [item for item in locked if isinstance(item, dict)]
    if len(packages) != len(locked):
        raise EvidenceError("Cargo.lock contains a malformed package record")
    keys = [_package_key(item) for item in packages]
    if len(set(keys)) != len(keys):
        raise EvidenceError("Cargo.lock contains duplicate package identities")
    lock_keys = set(keys)
    missing = sorted(identity for identity in workspace_members if
                     (identity[0], identity[1], None) not in lock_keys)
    if missing:
        raise EvidenceError(f"workspace crates absent from Cargo.lock: {missing[:10]}")

    lock_sha = _sha256(lock_bytes)
    root_package = workspace_manifest.get("package", {})
    root_name = root_package.get("name", "OpenATCA") if isinstance(root_package, dict) else "OpenATCA"
    root_version = root_package.get("version", "workspace") if isinstance(root_package, dict) else "workspace"
    doc_key = f"{root_name}-{root_version}-cargo-lock-{lock_sha}"
    spdx_ids = {key: "SPDXRef-Package-" + _sha256("\0".join((key[0], key[1], key[2] or "local")).encode())[:24]
                for key in keys}
    now = generated_at or datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    spdx_packages: list[dict[str, object]] = []
    third_party_count = 0
    for item in sorted(packages, key=lambda p: _package_key(p)):
        key = _package_key(item)
        source = key[2]
        member = workspace_members.get((key[0], key[1])) if source is None else None
        third_party = source is not None
        third_party_count += int(third_party)
        pkg: dict[str, object] = {
            "name": key[0], "SPDXID": spdx_ids[key], "versionInfo": key[1],
            "downloadLocation": source or "NOASSERTION", "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": member["license"] if member and member["license"] else "NOASSERTION",
            "copyrightText": "NOASSERTION",
            "packageComment": ("workspace member " + str(member["manifest_path"]) if member else
                               ("source-backed third-party package" if third_party else
                                "local Cargo.lock package outside workspace.members")),
        }
        checksum = item.get("checksum")
        if checksum is not None:
            if not isinstance(checksum, str) or not re.fullmatch(r"[0-9a-f]{64}", checksum):
                raise EvidenceError(f"invalid Cargo.lock checksum for {key[0]} {key[1]}")
            pkg["checksums"] = [{"algorithm": "SHA256", "checksumValue": checksum}]
        spdx_packages.append(pkg)

    relationships: set[tuple[str, str, str]] = set()
    for item in packages:
        parent_key = _package_key(item)
        deps = item.get("dependencies", [])
        if not isinstance(deps, list):
            raise EvidenceError(f"Cargo.lock dependencies are malformed for {parent_key[0]}")
        for dep in deps:
            child_key = _dependency_identity(dep, packages)
            relationships.add((spdx_ids[parent_key], "DEPENDS_ON", spdx_ids[child_key]))
    for identity in workspace_members:
        key = (identity[0], identity[1], None)
        relationships.add(("SPDXRef-DOCUMENT", "DESCRIBES", spdx_ids[key]))

    document = {
        "spdxVersion": "SPDX-2.3", "dataLicense": "CC0-1.0", "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"{root_name} {root_version} Cargo workspace lockfile SBOM",
        "documentNamespace": f"https://spdx.org/spdxdocs/{doc_key}",
        "creationInfo": {"creators": ["Tool: OpenATCA supply-chain evidence generator"],
                         "created": now,
                         "comment": f"Cargo.lock SHA256={lock_sha}; workspace members={len(workspace_members)}; source-backed third-party packages={third_party_count}."},
        "packages": spdx_packages,
        "relationships": [{"spdxElementId": left, "relationshipType": rel,
                            "relatedSpdxElement": right}
                           for left, rel, right in sorted(relationships)],
        "annotations": [{"annotationDate": now, "annotationType": "OTHER",
                         "annotator": "Tool: OpenATCA supply-chain evidence generator",
                         "SPDXID": "SPDXRef-DOCUMENT",
                         "comment": f"Third-party source package count is {third_party_count}; count is computed from Cargo.lock source fields and not presumed zero."}],
    }
    return document, third_party_count


def _run(argv: list[str], timeout: int = 30) -> str:
    try:
        completed = subprocess.run(argv, capture_output=True, timeout=timeout, check=False)
    except (OSError, subprocess.SubprocessError) as exc:
        raise EvidenceError(f"toolchain command failed ({argv[0]}): {type(exc).__name__}") from exc
    if completed.returncode != 0:
        raise EvidenceError(f"toolchain command refused ({argv[0]}), exit={completed.returncode}")
    try:
        return completed.stdout.decode("utf-8").strip()
    except UnicodeDecodeError as exc:
        raise EvidenceError(f"toolchain command output is not UTF-8 ({argv[0]})") from exc


def _parse_version_fields(text: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for line in text.splitlines():
        key, sep, value = line.partition(": ")
        if sep and key and value:
            fields[key] = value
    if not fields:
        raise EvidenceError("toolchain -vV output had no parseable fields")
    return fields


def _hash_file(path: Path) -> tuple[str, int]:
    if path.is_symlink():
        raise EvidenceError(f"refusing symlink toolchain component: {path}")
    before = path.stat()
    if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_COMPONENT_BYTES:
        raise EvidenceError(f"toolchain component is not a bounded regular file: {path}")
    digest = hashlib.sha256()
    count = 0
    with path.open("rb") as stream:
        while True:
            chunk = stream.read(1024 * 1024)
            if not chunk:
                break
            count += len(chunk)
            if count > MAX_COMPONENT_BYTES:
                raise EvidenceError(f"toolchain component grew beyond bound: {path}")
            digest.update(chunk)
    after = path.stat()
    if (count != before.st_size or
            (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) !=
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
        raise EvidenceError(f"toolchain component changed while hashing: {path}")
    return digest.hexdigest(), count


def _component_inventory(sysroot: Path) -> list[dict[str, object]]:
    rustlib = sysroot / "lib" / "rustlib"
    components_path = rustlib / "components"
    raw = _read_regular(components_path, MAX_METADATA_BYTES)
    names = raw.decode("utf-8").splitlines()
    if not names or len(names) != len(set(names)) or any(not SAFE_COMPONENT.fullmatch(name) for name in names):
        raise EvidenceError("rustup components inventory is empty, duplicate, or malformed")
    inventory = []
    total_files = total_bytes = 0
    for component in sorted(names):
        manifest = rustlib / f"manifest-{component}"
        manifest_bytes = _read_regular(manifest, MAX_METADATA_BYTES)
        entries = manifest_bytes.decode("utf-8").splitlines()
        paths = []
        for entry in entries:
            if not entry.startswith("file:"):
                raise EvidenceError(f"unsupported rustup component manifest row: {component}")
            relative = entry[5:]
            if (not relative or relative.startswith("/") or "\\" in relative or
                    any(part in ("", ".", "..") for part in relative.split("/"))):
                raise EvidenceError(f"unsafe rustup component path in {component}")
            paths.append(relative)
        if not paths or len(paths) > MAX_MANIFEST_FILES or len(paths) != len(set(paths)):
            raise EvidenceError(f"rustup component inventory invalid for {component}")
        component_digest = hashlib.sha256()
        component_bytes = 0
        entries_report = []
        for relative in sorted(paths):
            path = sysroot / relative
            try:
                path.resolve(strict=True).relative_to(sysroot)
            except (OSError, ValueError) as exc:
                raise EvidenceError(f"component path missing or escapes sysroot: {relative}") from exc
            file_digest, size = _hash_file(path)
            component_bytes += size
            total_bytes += size
            total_files += 1
            if total_files > MAX_MANIFEST_FILES * 16 or total_bytes > MAX_COMPONENT_BYTES:
                raise EvidenceError("installed Rust component set exceeds hashing limits")
            component_digest.update(relative.encode("utf-8") + b"\0" + file_digest.encode("ascii") + b"\n")
            entries_report.append({"path": relative, "sha256": file_digest, "bytes": size})
        inventory.append({"name": component, "manifest_sha256": _sha256(manifest_bytes),
                          "tree_sha256": component_digest.hexdigest(),
                          "file_count": len(entries_report), "bytes": component_bytes,
                          "files": entries_report})
    return inventory


def _command_binary(toolchain: str, command: str, sysroot: Path) -> dict[str, object]:
    output = _run(["rustup", "which", "--toolchain", toolchain, command])
    executable = Path(output)
    try:
        resolved = executable.resolve(strict=True)
        relative = resolved.relative_to(sysroot)
    except (OSError, ValueError) as exc:
        raise EvidenceError(f"{command} resolved outside the selected toolchain sysroot") from exc
    digest, size = _hash_file(resolved)
    return {"command": command, "path_under_sysroot": relative.as_posix(),
            "sha256": digest, "bytes": size}


def _git_provenance(root: Path) -> dict[str, object]:
    head = _run(["git", "-C", str(root), "rev-parse", "--verify", "HEAD"])
    if not re.fullmatch(r"[0-9a-f]{40,64}", head):
        raise EvidenceError("git HEAD is not a full commit id")
    # Git's -z status can contain arbitrary path bytes; hash stdout bytes instead of decoding filenames.
    status_result = subprocess.run(["git", "-C", str(root), "status", "--porcelain=v1", "-z", "--untracked-files=all"],
                                   capture_output=True, timeout=30, check=False)
    if status_result.returncode != 0:
        raise EvidenceError("git source status could not be read")
    raw_status = status_result.stdout
    if len(raw_status) > MAX_MANIFEST_BYTES:
        raise EvidenceError("git source status exceeds 2 MiB")
    if raw_status:
        raise EvidenceError("supply-chain evidence requires a clean source checkout")
    env_commit = os.environ.get("GITHUB_SHA")
    if env_commit and env_commit != head:
        raise EvidenceError("GitHub event commit does not match checked-out HEAD")
    invocation = {}
    for key in ("GITHUB_WORKFLOW", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_EVENT_NAME", "GITHUB_REF", "GITHUB_SHA"):
        value = os.environ.get(key)
        if value:
            invocation[key.lower()] = value
    return {"commit": head, "status": "clean", "status_bytes": 0,
            "status_sha256": _sha256(raw_status), "invocation": invocation}


def make_provenance(root: Path, toolchain: str, sbom_bytes: bytes) -> dict[str, object]:
    rustc_vv = _run(["rustc", f"+{toolchain}", "-vV"])
    cargo_vv = _run(["cargo", f"+{toolchain}", "-vV"])
    rustc_fields = _parse_version_fields(rustc_vv)
    cargo_fields = _parse_version_fields(cargo_vv)
    sysroot = Path(_run(["rustc", f"+{toolchain}", "--print", "sysroot"])).resolve(strict=True)
    binaries = [_command_binary(toolchain, command, sysroot) for command in TOOL_COMMANDS]
    rustup_path = Path(shutil.which("rustup") or "").resolve(strict=True)
    rustup_sha, rustup_bytes = _hash_file(rustup_path)
    os_release = Path("/etc/os-release")
    try:
        resolved_os_release = os_release.resolve(strict=True)
    except OSError as exc:
        raise EvidenceError("/etc/os-release is unavailable") from exc
    if not resolved_os_release.is_absolute():
        raise EvidenceError("resolved OS release path is not absolute")
    release_payload = _read_regular(resolved_os_release, MAX_METADATA_BYTES)
    release_values = {}
    for line in release_payload.decode("utf-8").splitlines():
        key, sep, value = line.partition("=")
        if sep:
            release_values[key] = value.strip('"')
    kernel = platform.uname()
    source = _git_provenance(root)
    now = datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    components = _component_inventory(sysroot)
    return {
        "schema": "openatca.toolchain-provenance.v1",
        "generated_at_utc": now,
        "evidence_kind": "unsigned local CI observation; not a signed build attestation",
        "source": {**source, "cargo_lock_sha256": _sha256(_read_regular(root / "Cargo.lock", MAX_LOCK_BYTES)),
                   "sbom_sha256": _sha256(sbom_bytes)},
        "toolchain": {
            "toolchain_selector": toolchain,
            "rustc_version_output": rustc_vv,
            "rustc_fields": rustc_fields,
            "cargo_version_output": cargo_vv,
            "cargo_fields": cargo_fields,
            "command_binaries": binaries,
            "rustup_binary": {"sha256": rustup_sha, "bytes": rustup_bytes},
            "sysroot_component_inventory_sha256": _sha256(json.dumps(components, sort_keys=True,
                                                                        separators=(",", ":")).encode("utf-8")),
            "components": components,
        },
        "host": {"os_release": {"path": str(os_release),
                                 "resolved_path": str(resolved_os_release),
                                 "id": release_values.get("ID", "unknown"),
                                 "version_id": release_values.get("VERSION_ID", "unknown"),
                                 "pretty_name": release_values.get("PRETTY_NAME", "unknown"),
                                 "sha256": _sha256(release_payload)},
                 "kernel": {"sysname": kernel.system, "release": kernel.release,
                            "version": kernel.version, "machine": kernel.machine}},
        "limitations": [
            "Hashes are measured from this runner after toolchain installation; they do not verify a vendor signature.",
            "No build signing key or FIPS-validated signing service is present; external signing and custody remain outstanding.",
            "This records toolchain and host inputs, not an independently witnessed build execution or reproducible-build comparison.",
        ],
    }


def _write_new(path: Path, payload: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


def generate(root: Path, output: Path, toolchain: str, require_zero_third_party: bool = True) -> int:
    root = root.resolve(strict=True)
    output = output.resolve(strict=False)
    if output == root or root in output.parents:
        raise EvidenceError("evidence output must be outside the source checkout")
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    try:
        sbom, third_party_count = make_spdx(root)
        sbom_bytes = (json.dumps(sbom, indent=2, sort_keys=True) + "\n").encode("utf-8")
        provenance = make_provenance(root, toolchain, sbom_bytes)
        provenance_bytes = (json.dumps(provenance, indent=2, sort_keys=True) + "\n").encode("utf-8")
        _write_new(output / "sbom.spdx.json", sbom_bytes)
        _write_new(output / "toolchain-provenance.json", provenance_bytes)
        print(json.dumps({"output": str(output), "sbom_sha256": _sha256(sbom_bytes),
                          "provenance_sha256": _sha256(provenance_bytes),
                          "workspace_package_count": sum(
                              1 for package in sbom["packages"] if str(package.get("packageComment", "")).startswith("workspace member ")),
                          "third_party_package_count": third_party_count}, sort_keys=True))
    except Exception:
        shutil.rmtree(output, ignore_errors=True)
        raise
    if require_zero_third_party and third_party_count != 0:
        raise EvidenceError(f"Cargo.lock contains {third_party_count} source-backed third-party packages; evidence was emitted but policy requires zero")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--toolchain", default="1.89.0")
    parser.add_argument("--allow-third-party", action="store_true",
                        help="emit evidence without enforcing the current zero-third-party policy")
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", args.toolchain):
        print("refused: toolchain must be an exact stable version", file=sys.stderr)
        return 2
    try:
        return generate(args.repo, args.out, args.toolchain,
                        require_zero_third_party=not args.allow_third_party)
    except (OSError, EvidenceError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        print(f"supply-chain evidence refused: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
