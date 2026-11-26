#!/usr/bin/env python3
"""Check F1a dependency provenance and all-kind workspace layering.

Run `cargo metadata --offline --no-deps --format-version 1` first. For an
external dependency, its exact Cargo.toml declaration must carry the adjacent
comment `# F1-APPROVAL: <specific approval reference>`. A generic justification
is not approval. Core crates may never depend on external packages, even with
that marker. Internal edges of every kind (normal, dev, build, target-specific)
participate in cycle and core/shell/tool checks.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

APPROVAL = re.compile(r"#\s*F1-APPROVAL:\s*([A-Za-z0-9][A-Za-z0-9._:/-]*)\s*$")
GENERIC_APPROVALS = {"approved", "approval", "justification", "yes", "true", "external"}
SECTION = re.compile(r"^\s*\[(.+)]\s*(?:#.*)?$")
ENTRY = re.compile(r"^\s*([A-Za-z0-9_-]+)\s*=")
VALID_CLASSES = {"core", "shell", "split", "unsplit"}


def _section_dependency_binding(
    section: str,
) -> tuple[bool, str | None, str | None, str | None] | None:
    """Return (workspace table, kind, target, nested dependency alias)."""
    workspace_nested = re.fullmatch(r"workspace\.dependencies\.([A-Za-z0-9_-]+)", section)
    if workspace_nested:
        return True, None, None, workspace_nested.group(1)
    if section == "workspace.dependencies":
        return True, None, None, None
    kinds = {"dependencies": "normal", "dev-dependencies": "dev", "build-dependencies": "build"}
    if section in kinds:
        return False, kinds[section], None, None
    nested = re.fullmatch(r"(dependencies|dev-dependencies|build-dependencies)\.([A-Za-z0-9_-]+)", section)
    if nested:
        return False, kinds[nested.group(1)], None, nested.group(2)
    match = re.fullmatch(
        r"target\.('(?:[^']*)'|\"(?:[^\"\\]|\\.)*\")\."
        r"(dependencies|dev-dependencies|build-dependencies)(?:\.([A-Za-z0-9_-]+))?",
        section,
    )
    if not match:
        return None
    try:
        target = tomllib.loads(f"target_value = {match.group(1)}")["target_value"]
    except tomllib.TOMLDecodeError:
        return None
    return False, kinds[match.group(2)], target, match.group(3)


def _dependency_declarations(manifest: Path) -> list[dict[str, str | bool | None]]:
    """Return exact alias/kind/target declarations and their adjacent F1 reference."""
    if not manifest.is_file():
        return []
    section = ""
    binding = None
    nested_row: dict[str, str | bool | None] | None = None
    found: list[dict[str, str | bool | None]] = []
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    for line in lines:
        header = SECTION.match(line)
        if header:
            section = header.group(1).strip().strip('"')
            binding = _section_dependency_binding(section)
            nested_row = None
            if binding and binding[3] is not None:
                workspace, kind, target, alias = binding
                match = APPROVAL.search(line)
                reference = match.group(1) if match else None
                if reference and reference.lower() in GENERIC_APPROVALS:
                    reference = None
                nested_row = {
                    "key": alias, "workspace": workspace, "kind": kind,
                    "target": target, "reference": reference, "inherited": False,
                }
                found.append(nested_row)
            continue
        if binding is None:
            continue
        if binding[3] is not None:
            if nested_row is not None and re.match(r"^\s*workspace\s*=\s*true\s*(?:#.*)?$", line):
                nested_row["inherited"] = True
            continue
        entry = ENTRY.match(line)
        if entry:
            match = APPROVAL.search(line)
            reference = match.group(1) if match else None
            if reference and reference.lower() in GENERIC_APPROVALS:
                reference = None
            workspace, kind, target, _ = binding
            found.append({
                "key": entry.group(1), "workspace": workspace, "kind": kind,
                "target": target, "reference": reference,
                "inherited": bool(re.search(r"\bworkspace\s*=\s*true\b", line.split("#", 1)[0])),
            })
    return found


def _approved(root: Path, manifest: Path, dep: dict[str, Any]) -> bool:
    """Require an approval adjacent to the exact direct or inherited declaration."""
    alias = dep.get("rename") or dep.get("name")
    kind = dep.get("kind") or "normal"
    target = dep.get("target")
    if not isinstance(alias, str):
        return False
    local = [
        row for row in _dependency_declarations(manifest)
        if row["key"] == alias and row["workspace"] is False
        and row["kind"] == kind and row["target"] == target and row["inherited"] is False
    ]
    if len(local) == 1:
        return bool(local[0]["reference"])
    inherited = [
        row for row in _dependency_declarations(manifest)
        if row["key"] == alias and row["workspace"] is False
        and row["kind"] == kind and row["target"] == target and row["inherited"] is True
    ]
    if len(inherited) != 1:
        return False
    root_rows = [
        row for row in _dependency_declarations(root / "Cargo.toml")
        if row["key"] == alias and row["workspace"] is True
    ]
    return len(root_rows) == 1 and bool(root_rows[0]["reference"])


def _relative(path: Path, root: Path) -> str | None:
    try:
        return path.resolve(strict=False).relative_to(root.resolve(strict=True)).as_posix()
    except (OSError, ValueError):
        return None


def inspect_metadata(metadata: dict[str, Any], root: Path) -> list[str]:
    errors: list[str] = []
    packages = metadata.get("packages")
    if not isinstance(packages, list) or not packages:
        return ["cargo metadata returned no package list"]
    root = root.resolve(strict=True)
    by_dir: dict[str, dict[str, Any]] = {}
    for package in packages:
        manifest = Path(package.get("manifest_path", ""))
        directory = _relative(manifest.parent, root) if manifest else None
        name = package.get("name")
        if not directory or not isinstance(name, str):
            errors.append(f"metadata package has invalid/outside manifest path: {name!r}")
            continue
        if directory in by_dir:
            errors.append(f"duplicate workspace package directory: {directory}")
            continue
        package_class = package.get("metadata", {}).get("openatca", {}).get("class")
        if package_class not in VALID_CLASSES:
            errors.append(f"{name}: missing/unknown package.metadata.openatca.class")
        by_dir[directory] = package

    graph: dict[str, list[tuple[str, str]]] = {name: [] for name in (p.get("name") for p in packages)}
    name_by_dir = {directory: p["name"] for directory, p in by_dir.items()}
    for package in packages:
        name = package.get("name", "<unknown>")
        manifest = Path(package.get("manifest_path", ""))
        source_class = package.get("metadata", {}).get("openatca", {}).get("class")
        for dep in package.get("dependencies", []):
            depname = dep.get("rename") or dep.get("name")
            if not isinstance(depname, str):
                errors.append(f"{name}: dependency with missing name")
                continue
            kind = dep.get("kind") or "normal"
            target = dep.get("target") or "all-targets"
            dep_path = dep.get("path")
            if dep_path:
                dep_dir = _relative(Path(dep_path), root)
                target_name = name_by_dir.get(dep_dir or "")
                if dep_dir is not None and target_name is None:
                    errors.append(f"{name} -> {depname} ({kind}, {target}): internal path does not resolve to a known workspace package/class")
                    continue
                if dep_dir is None:
                    if source_class == "core":
                        errors.append(f"{name} -> {depname} ({kind}, {target}): core-to-outside dependency forbidden (F6)")
                    elif not _approved(root, manifest, dep):
                        errors.append(f"{name} -> {depname} ({kind}, {target}): outside path lacks adjacent F1-APPROVAL reference")
                    continue
                target_package = by_dir[dep_dir]
                target_class = target_package.get("metadata", {}).get("openatca", {}).get("class")
                if target_class not in VALID_CLASSES:
                    errors.append(f"{name} -> {depname}: internal package has no known OpenATCA class")
                    continue
                if source_class == "core" and (
                    target_class != "core"
                    or "/tools/" in str(target_package.get("manifest_path", "")).replace("\\", "/")
                ):
                    errors.append(f"{name} -> {target_name} ({kind}, {target}): core-to-shell/tool edge forbidden")
                if target_name not in graph:
                    errors.append(f"{name} -> {depname}: internal path does not resolve to a metadata workspace package")
                    continue
                graph[name].append((target_name, f"{kind}/{target}"))
            else:
                # Registry/Git dependencies are external too. Cargo metadata
                # exposes a registry/source when present; a workspace package
                # without path is recognized by its package name.
                is_external = bool(dep.get("source") or dep.get("registry"))
                if is_external and source_class == "core":
                    errors.append(f"{name} -> {depname} ({kind}, {target}): core-to-outside dependency forbidden (F6)")
                elif is_external and not _approved(root, manifest, dep):
                    errors.append(f"{name} -> {depname} ({kind}, {target}): external dependency lacks adjacent F1-APPROVAL reference")
                elif not is_external and dep.get("name") not in {p.get("name") for p in packages}:
                    errors.append(f"{name} -> {depname} ({kind}, {target}): dependency is neither a workspace package nor an identified external source")

    # DFS reports each back-edge with its dependency kind; all Cargo edge kinds
    # are included, including dev/build and target-qualified edges.
    visiting: list[str] = []
    path_edges: list[str] = []
    active: set[str] = set()
    done: set[str] = set()

    def visit(node: str) -> None:
        if node in done:
            return
        active.add(node)
        visiting.append(node)
        for target, edge_kind in graph.get(node, []):
            if target in active:
                start = visiting.index(target)
                nodes = visiting[start:] + [target]
                edge_kinds = path_edges[start:] + [edge_kind]
                rendered = nodes[0]
                for edge_i, edge_name in enumerate(edge_kinds):
                    rendered += f" -[{edge_name}]-> {nodes[edge_i + 1]}"
                errors.append(f"dependency cycle: {rendered}")
            elif target not in done:
                path_edges.append(edge_kind)
                visit(target)
                path_edges.pop()
        visiting.pop()
        active.remove(node)
        done.add(node)

    for node in sorted(graph):
        visit(node)
    return sorted(set(errors))


def check_workspace(root: Path) -> list[str]:
    command = ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"]
    try:
        result = subprocess.run(command, cwd=root, text=True, capture_output=True, check=False)
    except OSError as error:
        return [f"cannot run {' '.join(command)}: {error}"]
    if result.returncode != 0:
        return [f"{' '.join(command)} failed: {result.stderr.strip()}"]
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        return [f"cargo metadata returned invalid JSON: {error}"]
    return inspect_metadata(metadata, root)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    args = parser.parse_args(argv)
    failures = check_workspace(args.root)
    if failures:
        print("F1a dependency graph check failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("F1a dependency graph check passed (offline metadata; all dependency kinds checked).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
