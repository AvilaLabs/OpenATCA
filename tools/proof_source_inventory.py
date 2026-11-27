"""Reconcile source-level Kani proof attributes with the registered proof inventory."""

from __future__ import annotations

import argparse
import ast
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import tomllib
from typing import Any

try:
    from . import quality_size_ratchet as ratchet
except ImportError:  # Running this file directly from the tools directory.
    import quality_size_ratchet as ratchet


MAX_SOURCE_FILES = 4096
MAX_SOURCE_BYTES = 8 * 1024 * 1024
MAX_SOURCE_HARNESSES = 512
MAX_REGISTRY_BYTES = 4 * 1024 * 1024
MAX_EXCLUSIONS_BYTES = 1024 * 1024
PROOF_ATTRIBUTE = ("#", "[", "kani", "::", "proof", "]")


class InventoryError(ValueError):
    """Input cannot be represented without guessing about proof coverage."""


@dataclass(frozen=True, order=True)
class SourceProof:
    package: str
    harness: str
    path: str
    line: int


def _read_bounded(path: Path, limit: int) -> str:
    info = path.stat(follow_symlinks=False)
    if not path.is_file() or path.is_symlink() or info.st_size > limit:
        raise InventoryError(f"source input is not a bounded regular file: {path}")
    with path.open("rb") as stream:
        raw = stream.read(limit + 1)
    if len(raw) > limit:
        raise InventoryError(f"source input exceeds its byte limit: {path}")
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise InventoryError(f"source input is not UTF-8: {path}") from error


def _package_name(package_root: Path) -> str:
    manifest = tomllib.loads(_read_bounded(package_root / "Cargo.toml", MAX_SOURCE_BYTES))
    package = manifest.get("package")
    name = package.get("name") if isinstance(package, dict) else None
    if not isinstance(name, str) or not name:
        raise InventoryError(f"package manifest has no package name: {package_root}")
    return name


def _file_module_prefix(source: Path, package_root: Path) -> tuple[str, ...]:
    relative = source.relative_to(package_root / "src").with_suffix("")
    parts = list(relative.parts)
    if parts[-1] in ("lib", "main", "mod"):
        parts.pop()
    if parts and parts[0] == "bin":
        parts = parts[2:] if len(parts) > 2 else []
    return tuple(parts)


def _module_bodies(tokens: list[ratchet.Token]) -> list[tuple[int, int, str]]:
    bodies = []
    for index, token in enumerate(tokens[:-2]):
        if token.value != "mod" or tokens[index + 1].kind != "identifier":
            continue
        boundary = next((cursor for cursor in range(index + 2, len(tokens))
                         if tokens[cursor].value in ("{", ";")), None)
        if boundary is None or tokens[boundary].value != "{":
            continue
        closing = ratchet._matching(tokens, boundary, "{", "}")
        if closing is None:
            raise InventoryError(f"unclosed inline module at line {token.line}")
        bodies.append((boundary, closing, tokens[index + 1].value.removeprefix("r#")))
    return bodies


def _preceding_attributes(tokens: list[ratchet.Token], index: int) -> list[list[ratchet.Token]]:
    cursor = index - 1
    if cursor >= 0 and tokens[cursor].value == ")":
        depth = 1
        cursor -= 1
        while cursor >= 0 and depth:
            depth += (tokens[cursor].value == ")") - (tokens[cursor].value == "(")
            cursor -= 1
        if cursor >= 0 and tokens[cursor].value == "pub":
            cursor -= 1
    elif cursor >= 0 and tokens[cursor].value == "pub":
        cursor -= 1
    attributes: list[list[ratchet.Token]] = []
    while cursor >= 2 and tokens[cursor].value == "]":
        depth = 1
        opening = cursor - 1
        while opening >= 0 and depth:
            depth += (tokens[opening].value == "]") - (tokens[opening].value == "[")
            opening -= 1
        marker = opening
        if marker < 0 or tokens[marker].value != "#":
            break
        attributes.append(tokens[opening + 2:cursor])
        cursor = marker - 1
    return attributes


def _path_overrides(source: Path, package_root: Path, prefix: tuple[str, ...],
                    tokens: list[ratchet.Token], modules: list[tuple[int, int, str]]) -> list[tuple[Path, tuple[str, ...]]]:
    overrides = []
    for index, token in enumerate(tokens[:-1]):
        if token.value != "mod" or tokens[index + 1].kind != "identifier":
            continue
        attributes = _preceding_attributes(tokens, index)
        path_attribute = next((attribute for attribute in attributes
                               if any(item.value == "path" for item in attribute)), None)
        if path_attribute is None:
            continue
        values = [item.value for item in path_attribute]
        if (len(values) != 3 or values[0] != "path" or values[1] != "="
                or path_attribute[2].kind != "literal"):
            raise InventoryError(f"unsupported path attribute at {source}:{token.line}")
        try:
            relative_path = ast.literal_eval(path_attribute[2].value)
        except (SyntaxError, ValueError) as error:
            raise InventoryError(f"invalid path attribute at {source}:{token.line}") from error
        if not isinstance(relative_path, str):
            raise InventoryError(f"non-string path attribute at {source}:{token.line}")
        inline = tuple(name for opening, closing, name in sorted(modules)
                       if opening < index < closing)
        target = (source.parent / relative_path).resolve()
        try:
            target.relative_to(package_root.resolve())
        except ValueError as error:
            raise InventoryError(f"path attribute escapes its package: {source}:{token.line}") from error
        overrides.append((target, (*prefix, *inline, tokens[index + 1].value.removeprefix("r#"))))
    return overrides


def _proof_function(tokens: list[ratchet.Token], attribute_start: int) -> tuple[str, int]:
    cursor = attribute_start + len(PROOF_ATTRIBUTE)
    while cursor < len(tokens) and tokens[cursor].value == "#":
        if cursor + 1 >= len(tokens) or tokens[cursor + 1].value != "[":
            raise InventoryError(f"malformed attribute after proof at line {tokens[attribute_start].line}")
        closing = ratchet._matching(tokens, cursor + 1, "[", "]")
        if closing is None:
            raise InventoryError(f"unclosed attribute after proof at line {tokens[attribute_start].line}")
        cursor = closing + 1
    while cursor < len(tokens) and tokens[cursor].value in {"pub", "async", "unsafe", "const", "extern"}:
        cursor += 1
    if (cursor + 1 >= len(tokens) or tokens[cursor].value != "fn"
            or tokens[cursor + 1].kind != "identifier"):
        raise InventoryError(f"proof attribute is not attached to a function at line {tokens[attribute_start].line}")
    return tokens[cursor + 1].value.removeprefix("r#"), cursor


def discover_source_proofs(root: Path) -> list[SourceProof]:
    roots = [root / "crates", root / "tools"]
    sources = sorted(path for base in roots if base.is_dir()
                     for path in base.glob("*/**/*.rs"))
    if len(sources) > MAX_SOURCE_FILES:
        raise InventoryError("source file limit exceeded")
    package_roots = {}
    overrides: dict[Path, tuple[str, ...]] = {}
    records = []
    for source in sources:
        package_root = next((parent for parent in source.parents if (parent / "Cargo.toml").is_file()), None)
        if package_root is None:
            raise InventoryError(f"Rust source has no package manifest: {source}")
        package = package_roots.get(package_root)
        if package is None:
            package = _package_name(package_root)
            package_roots[package_root] = package
        text = _read_bounded(source, MAX_SOURCE_BYTES)
        tokens = ratchet._rust_tokens(text)
        try:
            source.relative_to(package_root / "src")
        except ValueError:
            if any(tuple(token.value for token in tokens[index:index + len(PROOF_ATTRIBUTE)])
                   == PROOF_ATTRIBUTE
                   for index in range(len(tokens) - len(PROOF_ATTRIBUTE) + 1)):
                raise InventoryError(
                    f"Kani proof outside package src needs an explicit target mapping: {source}"
                )
            continue
        modules = _module_bodies(tokens)
        prefix = _file_module_prefix(source, package_root)
        for target, module_path in _path_overrides(source, package_root, prefix, tokens, modules):
            prior = overrides.get(target)
            if prior is not None and prior != module_path:
                raise InventoryError(f"source file is included under multiple module paths: {target}")
            overrides[target] = module_path
        records.append((source, package_root, package, prefix))
    found = []
    for source, package_root, package, prefix in records:
        tokens = ratchet._rust_tokens(_read_bounded(source, MAX_SOURCE_BYTES))
        modules = _module_bodies(tokens)
        module_prefix = overrides.get(source.resolve(), prefix)
        for index in range(len(tokens) - len(PROOF_ATTRIBUTE) + 1):
            if tuple(token.value for token in tokens[index:index + len(PROOF_ATTRIBUTE)]) != PROOF_ATTRIBUTE:
                continue
            name, function_index = _proof_function(tokens, index)
            inline = tuple(name for opening, closing, name in sorted(modules)
                           if opening < function_index < closing)
            found.append(SourceProof(package, "::".join((*module_prefix, *inline, name)),
                                     source.relative_to(root).as_posix(), tokens[index].line))
            if len(found) > MAX_SOURCE_HARNESSES:
                raise InventoryError("source proof harness limit exceeded")
    identities = [(item.package, item.harness) for item in found]
    if len(set(identities)) != len(identities):
        raise InventoryError("duplicate source package/harness identity")
    return sorted(found)


def _identity_rows(value: Any, label: str, *, exclusions: bool) -> dict[tuple[str, str], str]:
    if not isinstance(value, dict):
        raise InventoryError(f"{label} must be an object")
    rows = value.get("proofs" if not exclusions else "exclusions")
    if not isinstance(rows, list):
        raise InventoryError(f"{label} rows must be an array")
    result = {}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise InventoryError(f"{label}[{index}] must be an object")
        package, harness = row.get("package"), row.get("harness")
        if not isinstance(package, str) or not package or not isinstance(harness, str) or not harness:
            raise InventoryError(f"{label}[{index}] lacks package/harness identity")
        reason = row.get("reason", "")
        if exclusions and (not isinstance(reason, str) or not reason.strip()):
            raise InventoryError(f"{label}[{index}] requires a named reason")
        identity = (package, harness)
        if identity in result:
            raise InventoryError(f"duplicate {label} identity: {package} {harness}")
        result[identity] = reason
    return result


def compare_inventory(source: list[SourceProof], registry: Any, exclusions: Any = None) -> dict[str, Any]:
    registered = _identity_rows(registry, "registry", exclusions=False)
    excluded = _identity_rows(exclusions if exclusions is not None else {"exclusions": []},
                              "exclusions", exclusions=True)
    source_by_id = {(item.package, item.harness): item for item in source}
    overlap = set(registered) & set(excluded)
    if overlap:
        raise InventoryError("source identity is both registered and excluded")
    accounted = set(registered) | set(excluded)
    missing = sorted(set(source_by_id) - accounted)
    stale = sorted(accounted - set(source_by_id))
    return {
        "source_harness_count": len(source),
        "registered_source_harness_count": len(set(source_by_id) & set(registered)),
        "excluded_source_harness_count": len(set(source_by_id) & set(excluded)),
        "unaccounted_source_harnesses": [asdict(source_by_id[key]) for key in missing],
        "stale_registry_or_exclusion_identities": [
            {"package": package, "harness": harness} for package, harness in stale
        ],
        "status": "PASS" if not missing and not stale else "FAIL",
    }


def _validate_configuration(root: Path, registry_path: Path,
                            exclusions_path: Path | None) -> tuple[Path, Path, Path | None]:
    if root.is_symlink() or not root.is_dir():
        raise InventoryError("workspace root is missing, not a directory, or a symlink")
    resolved = root.resolve()
    if resolved == Path("/"):
        raise InventoryError("filesystem root cannot be an inventory workspace")
    workspace = tomllib.loads(_read_bounded(resolved / "Cargo.toml", MAX_SOURCE_BYTES)).get("workspace")
    members = workspace.get("members") if isinstance(workspace, dict) else None
    if (not isinstance(members, list) or "crates/oa-track-core" not in members
            or "tools/oa-trace" not in members):
        raise InventoryError("workspace root does not match the OpenATCA workspace layout")
    for marker in (resolved / "crates/oa-track-core/Cargo.toml",
                   resolved / "tools/oa-trace/Cargo.toml"):
        if not marker.is_file() or marker.is_symlink():
            raise InventoryError("workspace identity marker is missing or invalid")
    selected_registry = registry_path if registry_path.is_absolute() else resolved / registry_path
    expected_registry = resolved / "tools/rust_proofs.json"
    if selected_registry.resolve() != expected_registry.resolve():
        raise InventoryError("registry path is not the workspace proof registry")
    selected_exclusions = exclusions_path
    if exclusions_path is not None:
        selected_exclusions = (exclusions_path if exclusions_path.is_absolute()
                               else resolved / exclusions_path)
        if selected_exclusions.resolve().parent != (resolved / "tools").resolve():
            raise InventoryError("exclusions file is outside the workspace tools directory")
    return resolved, expected_registry, selected_exclusions


def check_paths(root: Path, registry_path: Path, exclusions_path: Path | None = None) -> dict[str, Any]:
    root, registry_path, exclusions_path = _validate_configuration(
        root, registry_path, exclusions_path
    )
    registry = json.loads(_read_bounded(registry_path, MAX_REGISTRY_BYTES))
    exclusions = None if exclusions_path is None else json.loads(
        _read_bounded(exclusions_path, MAX_EXCLUSIONS_BYTES)
    )
    return compare_inventory(discover_source_proofs(root), registry, exclusions)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--exclusions", type=Path)
    args = parser.parse_args(argv)
    try:
        result = check_paths(args.root, args.registry, args.exclusions)
    except (InventoryError, OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(json.dumps({"status": "FAIL", "error": str(error)}, sort_keys=True))
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
