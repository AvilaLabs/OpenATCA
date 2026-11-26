#!/usr/bin/env python3
"""Create a private source-and-test staging tree for a reviewed CORE 0.1 package selection.

This tool copies no build outputs, recordings, or datasets. It never publishes. The generated
manifests are package-name/dependency transformations of one clean, committed workspace revision;
tracked source, tests, approved fixtures, and API pages are byte-for-byte copies. Any unsupported
dependency or source layout stops staging instead of dropping that requirement.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib


ROOT_CANDIDATES = (
    "oa-ieee", "oa-sha256", "oa-tar", "oa-xml", "oa-json", "oa-csv",
    "oa-math", "oa-geo", "oa-deflate", "oa-zip", "oa-asterix", "oa-arinc424",
    "oa-nasr", "oa-notam", "oa-fixm", "oa-grib2",
)
API_DOCS = {
    "oa-ieee": "docs/core-api/foundations/math-ieee.md",
    "oa-math": "docs/core-api/foundations/math-ieee.md",
    "oa-geo": "docs/core-api/foundations/geo.md",
    "oa-time-window": "docs/core-api/foundations/time-window.md",
    "oa-sha256": "docs/core-api/foundations/integrity.md",
    "oa-deflate": "docs/core-api/foundations/containers.md",
    "oa-zip": "docs/core-api/foundations/containers.md",
    "oa-tar": "docs/core-api/foundations/containers.md",
    "oa-xml": "docs/core-api/foundations/text.md",
    "oa-json": "docs/core-api/foundations/text.md",
    "oa-csv": "docs/core-api/foundations/text.md",
    "oa-asterix": "docs/core-api/formats/asterix.md",
    "oa-arinc424": "docs/core-api/formats/arinc424.md",
    "oa-nasr": "docs/core-api/formats/nasr.md",
    "oa-notam": "docs/core-api/formats/aixm-notam.md",
    "oa-fixm": "docs/core-api/formats/fixm-sfdps.md",
    "oa-grib2": "docs/core-api/formats/grib2-winds.md",
    "oa-j2k": "docs/core-api/formats/grib2-winds.md",
    "oa-airspace": "docs/core-api/modules/airspace.md",
    "oa-adsb": "crates/oa-adsb/README.md",
}
PUBLIC_PREFIX = "openatca-"
BLOCKED_PACKAGES = {"oa-sealed"}
MAX_SOURCE_FILE = 32 * 1024 * 1024
MAX_SOURCE_TOTAL = 512 * 1024 * 1024
TEST_ASSETS = {
    "oa-sha256": {
        "tests/fixtures/SHA256LongMsg.rsp",
        "tests/fixtures/SHA256Monte.rsp",
        "tests/fixtures/SHA256ShortMsg.rsp",
    },
    "oa-xml": set(),
}
COMPILE_TIME_MACRO = re.compile(rb'\b(?:include_(?:bytes|str)|include)!\s*\(([^)]*)\)')
COMPILE_TIME_LITERAL = re.compile(rb'^\s*"([^\"]+)"\s*$')
PATH_ATTRIBUTE = re.compile(rb'#\[path\s*=\s*"([^\"]+)"\s*\]')


class StageError(Exception):
    """A source or package boundary that needs review before staging."""


def git(repo: Path, *args: str, binary: bool = False) -> bytes | str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], check=False, capture_output=True,
        text=not binary, encoding=None if binary else "utf-8",
    )
    if result.returncode != 0:
        error = result.stderr if isinstance(result.stderr, str) else result.stderr.decode("utf-8", "replace")
        raise StageError(f"git {' '.join(args)} failed: {error.strip()}")
    return result.stdout


def source_bytes(repo: Path, revision: str, relative: str) -> bytes:
    if relative.startswith("/") or ".." in PurePosixPath(relative).parts:
        raise StageError(f"unsafe repository path: {relative}")
    value = git(repo, "show", f"{revision}:{relative}", binary=True)
    if not isinstance(value, bytes):
        raise StageError(f"git did not return binary source for {relative}")
    if len(value) > MAX_SOURCE_FILE:
        raise StageError(f"source file exceeds {MAX_SOURCE_FILE} bytes: {relative}")
    return value


def public_name(internal_name: str) -> str:
    if not internal_name.startswith("oa-") or internal_name in BLOCKED_PACKAGES:
        raise StageError(f"no public package mapping for internal dependency {internal_name}")
    return PUBLIC_PREFIX + internal_name[3:]


def rust_crate_name(manifest: dict, internal_name: str) -> str:
    lib_target = manifest.get("lib")
    if lib_target is None:
        return internal_name.replace("-", "_")
    if not isinstance(lib_target, dict):
        raise StageError(f"unsupported library target declaration for {internal_name}")
    target_name = lib_target.get("name")
    if target_name is None:
        return internal_name.replace("-", "_")
    if not isinstance(target_name, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", target_name):
        raise StageError(f"unsupported Rust library import name for {internal_name}: {target_name!r}")
    return target_name


def dependency_tables(manifest: dict) -> list[tuple[str, str, object]]:
    found: list[tuple[str, str, object]] = []
    for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        section = manifest.get(section_name, {})
        if isinstance(section, dict):
            found.extend((section_name, alias, value) for alias, value in section.items())
    target = manifest.get("target", {})
    if isinstance(target, dict):
        for target_cfg in target.values():
            if not isinstance(target_cfg, dict):
                continue
            for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
                section = target_cfg.get(section_name, {})
                if isinstance(section, dict):
                    found.extend((f"target.{section_name}", alias, value) for alias, value in section.items())
    return found


def resolve_package(repo: Path, revision: str, relative: str, cache: dict[str, tuple[dict, bytes]]) -> tuple[dict, bytes]:
    manifest_path = f"{relative}/Cargo.toml"
    if manifest_path not in cache:
        raw = source_bytes(repo, revision, manifest_path)
        try:
            parsed = tomllib.loads(raw.decode("utf-8"))
        except (UnicodeError, tomllib.TOMLDecodeError) as exc:
            raise StageError(f"cannot parse {manifest_path}: {exc}") from exc
        cache[manifest_path] = (parsed, raw)
    return cache[manifest_path]


def dependency_graph(repo: Path, revision: str, root_candidates: tuple[str, ...]) -> tuple[
        dict[str, str], dict[str, set[str]], dict[str, tuple[dict, bytes]], list[str]]:
    cache: dict[str, tuple[dict, bytes]] = {}
    module_paths: dict[str, str] = {}
    graph: dict[str, set[str]] = {}
    blockers: list[str] = []
    if not root_candidates or len(set(root_candidates)) != len(root_candidates):
        raise StageError("package selection must be nonempty and contain no duplicates")
    unknown = sorted(set(root_candidates) - set(ROOT_CANDIDATES))
    if unknown:
        raise StageError(f"package selection contains unsupported roots: {', '.join(unknown)}")
    pending = list(root_candidates)
    while pending:
        module = pending.pop()
        if module in module_paths:
            continue
        if module in BLOCKED_PACKAGES:
            raise StageError(f"public closure contains prohibited internal package {module}")
        relative = f"crates/{module}"
        parsed, _ = resolve_package(repo, revision, relative, cache)
        package = parsed.get("package")
        if not isinstance(package, dict) or package.get("name") != module:
            raise StageError(f"package identity mismatch at {relative}/Cargo.toml")
        module_paths[module] = relative
        graph[module] = set()
        for section_name, alias, dep in dependency_tables(parsed):
            if not isinstance(dep, dict):
                blockers.append(f"{module}: dependency needs an explicit package table: {alias}")
                continue
            if dep.get("workspace") is True:
                blockers.append(f"{module}: workspace-inherited dependency needs explicit staging support: {alias}")
                continue
            path_value = dep.get("path")
            if path_value is None:
                blockers.append(f"{module}: unresolved or external dependency: {alias}")
                continue
            if not isinstance(path_value, str):
                blockers.append(f"{module}: unsupported dependency path: {alias}")
                continue
            resolved = PurePosixPath(relative).joinpath(path_value)
            normalized = PurePosixPath(os.path.normpath(str(resolved)))
            if normalized.is_absolute() or not normalized.parts or normalized.parts[0] != "crates":
                blockers.append(f"{module}: dependency escapes crate tree: {alias} -> {path_value}")
                continue
            target_relative = normalized.as_posix()
            try:
                target_manifest, _ = resolve_package(repo, revision, target_relative, cache)
            except StageError:
                blockers.append(f"{module}: dependency manifest missing: {alias} -> {target_relative}")
                continue
            target_package = target_manifest.get("package")
            target_name = target_package.get("name") if isinstance(target_package, dict) else None
            if not isinstance(target_name, str):
                blockers.append(f"{module}: dependency package name missing: {alias} -> {target_relative}")
                continue
            if target_name in BLOCKED_PACKAGES:
                blockers.append(f"{module}: prohibited private {section_name} dependency {alias} -> {target_name}")
                continue
            if not target_name.startswith("oa-"):
                blockers.append(f"{module}: unexpected non-OpenATCA path dependency: {alias} -> {target_name}")
                continue
            graph[module].add(target_name)
            if target_name not in module_paths:
                pending.append(target_name)
    return module_paths, graph, cache, blockers


def topological_order(graph: dict[str, set[str]]) -> list[str]:
    order: list[str] = []
    active: set[str] = set()
    done: set[str] = set()

    def visit(name: str) -> None:
        if name in done:
            return
        if name in active:
            raise StageError(f"cyclic package dependency involving {name}")
        active.add(name)
        for dependency in sorted(graph.get(name, ())):
            if dependency not in graph:
                raise StageError(f"dependency missing from generated closure: {name} -> {dependency}")
            visit(dependency)
        active.remove(name)
        done.add(name)
        order.append(name)

    for package in sorted(graph):
        visit(package)
    return order


def transform_manifest(raw: bytes, internal_name: str, dependency_paths: dict[str, str],
                       workspace_package: dict, workspace_lints: dict,
                       library_name: str) -> bytes:
    text = raw.decode("utf-8")
    replacements = {
        rf'(?m)^name\s*=\s*"{re.escape(internal_name)}"\s*$': f'name = "{public_name(internal_name)}"',
        r'(?m)^version\.workspace\s*=\s*true\s*$': 'version = "0.1.0"',
    }
    for pattern, replacement in replacements.items():
        text, count = re.subn(pattern, replacement, text, count=1)
        if count != 1:
            raise StageError(f"could not transform manifest identity for {internal_name}")
    inherited_fields = {
        "edition": workspace_package.get("edition"),
        "license": workspace_package.get("license"),
        "rust-version": workspace_package.get("rust-version"),
        "authors": workspace_package.get("authors"),
    }
    for field, value in inherited_fields.items():
        if value is None:
            raise StageError(f"workspace package metadata lacks {field}")
        encoded = json.dumps(value, separators=(",", ":"))
        text, count = re.subn(rf'(?m)^{re.escape(field)}\.workspace\s*=\s*true\s*$',
                              f"{field} = {encoded}", text, count=1)
        if count != 1:
            raise StageError(f"could not expand inherited {field} metadata for {internal_name}")
    if workspace_lints:
        text, lint_count = re.subn(r'(?m)^\[lints\]\s*\nworkspace\s*=\s*true\s*$',
                                   "[lints.rust]\nunsafe_code = \"forbid\"\n"
                                   "unexpected_cfgs = { level = \"warn\", check-cfg = [\"cfg(kani)\", \"cfg(kani_full_capacity)\", \"cfg(kani_late_replay_covers)\", \"cfg(creusot)\"] }\n\n"
                                   "[lints.clippy]\nunwrap_used = \"deny\"\nexpect_used = \"deny\"\npanic = \"deny\"\nindexing_slicing = \"deny\"",
                                   text, count=1)
        if "[lints]\nworkspace = true" in text or lint_count not in (0, 1):
            raise StageError(f"could not expand inherited lints for {internal_name}")
    text, readme_count = re.subn(r'(?m)^readme\s*=.*$', 'readme = "README.md"', text, count=1)
    if readme_count == 0:
        marker = "[package]\n"
        if marker not in text:
            raise StageError(f"cannot add package README field for {internal_name}")
        text = text.replace(marker, marker + 'readme = "README.md"\n', 1)
    include_value = 'include = ["Cargo.toml", "src/*.rs", "src/**/*.rs", "build.rs", "examples/**/*.rs", "tests/*.rs", "tests/**/*.rs", "tests/fixtures/*", "README.md", "API.md", "LICENSE"]'
    if re.search(r'(?m)^include\s*=', text):
        text, count = re.subn(r'(?m)^include\s*=.*$', include_value, text, count=1)
        if count != 1:
            raise StageError(f"cannot scope package contents for {internal_name}")
    else:
        text, count = re.subn(r'(?m)^readme\s*=\s*"README\.md"\s*$',
                              'readme = "README.md"\n' + include_value, text, count=1)
        if count != 1:
            raise StageError(f"cannot scope package contents for {internal_name}")

    path_pattern = re.compile(r'(?P<left>\bpath\s*=\s*)"(?P<path>[^"]+)"')

    def replace_path(match: re.Match[str]) -> str:
        old_path = match.group("path")
        target = dependency_paths.get(old_path)
        if target is None:
            raise StageError(f"manifest dependency path is absent from parsed graph: {internal_name}:{old_path}")
        target_name = Path(target).name
        return (f'{match.group("left")}"../{target_name}", '
                f'package = "{target_name}", version = "0.1.0"')

    path_count = len(re.findall(r'\bpath\s*=', text))
    text, converted = path_pattern.subn(replace_path, text)
    if path_count != converted:
        raise StageError(f"unsupported path dependency form in {internal_name}: {path_count} path(s), {converted} converted")
    lib_header = re.search(r'(?m)^\[lib\]\s*$', text)
    if lib_header is None:
        text = text.rstrip() + f'\n\n[lib]\nname = {json.dumps(library_name)}\n'
    else:
        section_start = lib_header.end()
        next_section = re.search(r'(?m)^\[', text[section_start:])
        section_end = section_start + next_section.start() if next_section else len(text)
        lib_body = text[section_start:section_end]
        name_match = re.search(r'(?m)^name\s*=\s*"[^"]*"\s*$', lib_body)
        if name_match is None:
            text = text[:section_start] + f'\nname = {json.dumps(library_name)}\n' + text[section_start:]
        else:
            text = (text[:section_start + name_match.start()]
                    + f'name = {json.dumps(library_name)}'
                    + text[section_start + name_match.end():])
    return (text.rstrip() + "\n").encode("utf-8")


def git_files(repo: Path, revision: str, crate_relative: str) -> tuple[list[tuple[str, str]], list[str]]:
    raw = git(repo, "ls-tree", "-r", "-z", "--full-tree", revision, "--", crate_relative, binary=True)
    if not isinstance(raw, bytes):
        raise StageError(f"git did not return binary tree metadata for {crate_relative}")
    records: list[tuple[str, str]] = []
    omitted: list[str] = []
    for entry in raw.split(b"\0"):
        if not entry:
            continue
        metadata, path_bytes = entry.split(b"\t", 1)
        mode, kind, _object_id = metadata.decode("ascii").split(" ")
        path = path_bytes.decode("utf-8")
        if kind != "blob" or mode not in {"100644", "100755"}:
            raise StageError(f"non-regular package input is not supported: {path}")
        if path == f"{crate_relative}/Cargo.toml" or path == f"{crate_relative}/build.rs":
            records.append((path, mode))
        elif path.startswith(f"{crate_relative}/src/"):
            if not path.endswith(".rs"):
                raise StageError(f"non-Rust production source needs explicit review: {path}")
            records.append((path, mode))
        elif path.startswith(f"{crate_relative}/examples/"):
            if not path.endswith(".rs"):
                raise StageError(f"non-Rust example needs explicit review: {path}")
            records.append((path, mode))
        elif path.startswith(f"{crate_relative}/tests/"):
            inside = path[len(crate_relative) + 1:]
            if path.endswith(".rs") or inside in TEST_ASSETS.get(Path(crate_relative).name, set()):
                records.append((path, mode))
            else:
                omitted.append(path)
        elif path == f"{crate_relative}/README.md":
            records.append((path, mode))
        else:
            omitted.append(path)
    if not any(path == f"{crate_relative}/Cargo.toml" for path, _ in records):
        raise StageError(f"package manifest is not tracked at {crate_relative}/Cargo.toml")
    if not any(path.startswith(f"{crate_relative}/src/") for path, _ in records):
        raise StageError(f"package has no tracked Rust source under {crate_relative}/src")
    if not any(path == f"{crate_relative}/README.md" for path, _ in records):
        raise StageError(f"package README is not tracked at {crate_relative}/README.md")
    return sorted(records), sorted(omitted)


def write_private(path: Path, data: bytes) -> str:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with path.open("xb") as stream:
        os.chmod(path, 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    return hashlib.sha256(data).hexdigest()


def fsync_directories(root: Path) -> None:
    directories = [root, *(path for path in root.rglob("*") if path.is_dir())]
    for directory in sorted(directories, key=lambda item: len(item.parts), reverse=True):
        fd = os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def stage(repo_arg: Path, revision: str, output: Path,
          root_candidates: tuple[str, ...] = ROOT_CANDIDATES) -> dict:
    repo = Path(git(repo_arg, "rev-parse", "--show-toplevel").strip()).resolve()
    head = git(repo, "rev-parse", "HEAD").strip()
    commit = git(repo, "rev-parse", f"{revision}^{{commit}}").strip()
    if head != commit:
        raise StageError("staging requires HEAD to equal the requested full source commit")
    status = git(repo, "status", "--porcelain=v1", "--untracked-files=all")
    if status:
        raise StageError("staging requires a clean checkout; no files were copied")
    destination = output.absolute()
    if destination.exists() or destination.is_symlink():
        raise StageError("staging destination must be a new path")
    try:
        destination.relative_to(repo)
    except ValueError:
        pass
    else:
        raise StageError("staging destination must be outside the source repository")
    if not destination.parent.is_dir():
        raise StageError("staging destination parent must already exist and be a real directory")
    if destination.parent.is_symlink():
        raise StageError("staging destination parent path must not traverse symlink components")
    if destination.parent.resolve(strict=True) != destination.parent:
        raise StageError("staging destination parent path must not traverse symlink components")

    modules, graph, parsed_cache, dependency_blockers = dependency_graph(repo, commit, root_candidates)
    order = topological_order(graph)
    library_names = {
        module: rust_crate_name(parsed_cache[f"{modules[module]}/Cargo.toml"][0], module)
        for module in order
    }
    root_cargo = source_bytes(repo, commit, "Cargo.toml")
    try:
        root_manifest = tomllib.loads(root_cargo.decode("utf-8"))
        workspace = root_manifest["workspace"]
        workspace_package = workspace["package"]
        workspace_lints = workspace["lints"]
    except (UnicodeError, tomllib.TOMLDecodeError, KeyError, TypeError) as exc:
        raise StageError(f"source workspace metadata is incomplete: {exc}") from exc
    expected_lints = {
        "rust": {
            "unsafe_code": "forbid",
            "unexpected_cfgs": {
                "level": "warn",
                "check-cfg": ["cfg(kani)", "cfg(kani_full_capacity)", "cfg(kani_late_replay_covers)", "cfg(creusot)"],
            },
        },
        "clippy": {"unwrap_used": "deny", "expect_used": "deny", "panic": "deny", "indexing_slicing": "deny"},
    }
    if workspace_lints != expected_lints:
        raise StageError("workspace lint inheritance changed; update and review the staging transformation")

    file_plans: dict[str, list[tuple[str, str]]] = {}
    blockers: list[str] = list(dependency_blockers)
    for module in order:
        source_dir = modules[module]
        files, omitted = git_files(repo, commit, source_dir)
        file_plans[module] = files
        blockers.extend(f"{module}: tracked package input is not staged: {path}" for path in omitted)
        available = {path for path, _ in files}
        for source_path, _mode in files:
            if not source_path.endswith(".rs"):
                continue
            payload = source_bytes(repo, commit, source_path)
            references: list[tuple[str, str]] = []
            for match in COMPILE_TIME_MACRO.finditer(payload):
                literal = COMPILE_TIME_LITERAL.fullmatch(match.group(1))
                if literal is None:
                    blockers.append(f"{module}: {source_path} has a nonliteral compile-time include requiring review")
                    continue
                references.append((literal.group(1).decode("utf-8", "strict"), "compile-time include"))
            references.extend((match.group(1).decode("utf-8", "strict"), "path attribute")
                              for match in PATH_ATTRIBUTE.finditer(payload))
            for include_name, reference_kind in references:
                if PurePosixPath(include_name).is_absolute():
                    blockers.append(f"{module}: {source_path} uses absolute {reference_kind} {include_name}")
                    continue
                referenced = PurePosixPath(source_path).parent.joinpath(include_name)
                normalized_path = PurePosixPath(os.path.normpath(str(referenced)))
                normalized = normalized_path.as_posix()
                if ".." in normalized_path.parts or not normalized.startswith(f"{source_dir}/"):
                    blockers.append(f"{module}: {source_path} {reference_kind} escapes package: {include_name}")
                    continue
                if normalized not in available:
                    blockers.append(f"{module}: {source_path} {reference_kind} references unstaged file {normalized}")
        api_path = API_DOCS.get(module)
        if api_path is None:
            blockers.append(f"{module}: no source-backed API document mapping")
        else:
            try:
                source_bytes(repo, commit, api_path)
            except StageError as exc:
                blockers.append(f"{module}: API document unavailable: {exc}")
    if blockers:
        preview = "\n".join(f"- {item}" for item in sorted(set(blockers)))
        raise StageError("release staging is incomplete; no output was created:\n" + preview)

    os.mkdir(destination, 0o700)
    source_inventory: dict[str, str] = {"Cargo.toml": hashlib.sha256(root_cargo).hexdigest()}
    output_inventory: dict[str, str] = {}
    total = len(root_cargo)
    try:
        members = [f"crates/{public_name(module)}" for module in order]
        workspace_text = (
            "[workspace]\nresolver = \"2\"\nmembers = "
            + json.dumps(members, separators=(",", ":")) + "\n\n[workspace.package]\n"
            + f'version = "0.1.0"\nedition = {json.dumps(workspace_package["edition"])}\n'
            + f'license = {json.dumps(workspace_package["license"])}\n'
            + f'rust-version = {json.dumps(workspace_package["rust-version"])}\n'
            + f'authors = {json.dumps(workspace_package["authors"], separators=(",", ":"))}\n\n'
            + "[workspace.lints.rust]\nunsafe_code = \"forbid\"\n"
            + "unexpected_cfgs = { level = \"warn\", check-cfg = [\"cfg(kani)\", \"cfg(kani_full_capacity)\", \"cfg(kani_late_replay_covers)\", \"cfg(creusot)\"] }\n\n"
            + "[workspace.lints.clippy]\nunwrap_used = \"deny\"\nexpect_used = \"deny\"\npanic = \"deny\"\nindexing_slicing = \"deny\"\n"
        ).encode("utf-8")
        output_inventory["Cargo.toml"] = write_private(destination / "Cargo.toml", workspace_text)
        total += len(workspace_text)

        for module in order:
            source_dir = modules[module]
            package_dir = f"crates/{public_name(module)}"
            parsed, original_manifest = parsed_cache[f"{source_dir}/Cargo.toml"]
            dependency_paths: dict[str, str] = {}
            for _section_name, _alias, dep in dependency_tables(parsed):
                old_path = dep.get("path")
                if not isinstance(old_path, str):
                    continue
                resolved = PurePosixPath(source_dir).joinpath(old_path)
                normalized = PurePosixPath(os.path.normpath(str(resolved))).as_posix()
                dep_package = parsed_cache.get(f"{normalized}/Cargo.toml")
                if dep_package is None:
                    dep_manifest, _ = resolve_package(repo, commit, normalized, parsed_cache)
                else:
                    dep_manifest = dep_package[0]
                dep_info = dep_manifest.get("package", {})
                dep_name = dep_info.get("name") if isinstance(dep_info, dict) else None
                if not isinstance(dep_name, str) or dep_name not in modules:
                    raise StageError(f"dependency outside release closure while transforming {module}: {old_path}")
                dependency_paths[old_path] = f"crates/{public_name(dep_name)}"

            files = file_plans[module]
            for source_path, mode in files:
                relative_inside = source_path[len(source_dir) + 1:]
                payload = source_bytes(repo, commit, source_path)
                total += len(payload)
                if total > MAX_SOURCE_TOTAL:
                    raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
                source_inventory[source_path] = hashlib.sha256(payload).hexdigest()
                if relative_inside == "Cargo.toml":
                    payload = transform_manifest(original_manifest, module, dependency_paths,
                                                 workspace_package, workspace_lints, library_names[module])
                target_path = Path(package_dir) / relative_inside
                digest = write_private(destination / target_path, payload)
                output_inventory[target_path.as_posix()] = digest
                if mode == "100755":
                    os.chmod(destination / target_path, 0o700)
            api_path = API_DOCS.get(module)
            if not api_path:
                raise StageError(f"no source-backed API document mapped for package {module}")
            api_source_data = source_bytes(repo, commit, api_path)
            api_mapping = (
                f"# Package and Rust import names\n\n"
                f"Cargo package: `{public_name(module)}`  \n"
                f"Rust crate import: `{library_names[module]}`\n\n"
                "The package name used by Cargo and the Rust crate name used in source imports "
                "are intentionally recorded separately for review.\n\n---\n\n"
            ).encode("utf-8")
            api_data = api_mapping + api_source_data
            total += len(api_data)
            if total > MAX_SOURCE_TOTAL:
                raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
            source_inventory[api_path] = hashlib.sha256(api_source_data).hexdigest()
            package_api = Path(package_dir) / "API.md"
            output_inventory[package_api.as_posix()] = write_private(destination / package_api, api_data)
            package_license = Path(package_dir) / "LICENSE"
            license_data = source_bytes(repo, commit, "LICENSE")
            total += len(license_data)
            if total > MAX_SOURCE_TOTAL:
                raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
            source_inventory["LICENSE"] = hashlib.sha256(license_data).hexdigest()
            output_inventory[package_license.as_posix()] = write_private(destination / package_license, license_data)

        if total > MAX_SOURCE_TOTAL:
            raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
        manifest = {
            "schema": "openatca-core-release-stage-v1",
            "source_commit": commit,
            "package_version": "0.1.0",
            "package_order": [public_name(module) for module in order],
            "package_import_names": {
                public_name(module): library_names[module] for module in order
            },
            "dependency_graph": {public_name(module): sorted(public_name(dep) for dep in graph[module]) for module in order},
            "source_files_sha256": dict(sorted(source_inventory.items())),
            "staged_files_sha256": dict(sorted(output_inventory.items())),
            "content_scope": "tracked Rust source, test code, allowlisted test fixtures, generated package manifests, Apache license, and mapped API markdown; no recordings, datasets, or build outputs",
            "test_scope": "Tracked tests and explicitly allowlisted compile-time fixtures are copied byte-for-byte; staging does not execute tests or claim qualification.",
            "package_completeness": "source-and-test package preview for the selected roots and dependency closure; publication and release qualification remain unapproved",
            "publication_state": "not-published; review and gated validation required",
        }
        manifest_data = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode("utf-8")
        write_private(destination / "release-manifest.json", manifest_data)
        fsync_directories(destination)
        return manifest
    except Exception:
        # A partial staging tree is retained for diagnosis, with restrictive permissions.
        fsync_directories(destination)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True, help="clean source worktree")
    parser.add_argument("--commit", required=True, help="full source commit to stage")
    parser.add_argument("--output", type=Path, required=True, help="new private staging directory outside the repository")
    parser.add_argument("--packages", help="comma-separated release roots from the reviewed candidate set; default: all roots")
    args = parser.parse_args()
    try:
        roots = tuple(item.strip() for item in args.packages.split(",")) if args.packages else ROOT_CANDIDATES
        manifest = stage(args.repo, args.commit, args.output, roots)
    except (StageError, OSError, ValueError, TypeError, KeyError) as exc:
        print(f"oa-core-release-stage: {exc}", file=sys.stderr)
        return 2
    print(f"prepared source-and-test preview for {len(manifest['package_order'])} packages from {manifest['source_commit']}")
    print("only tracked source, tests, approved fixtures, and API docs are copied; no recordings or datasets")
    print("this is a source package preview, not a qualification or publication claim")
    print(f"manifest: {args.output / 'release-manifest.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
