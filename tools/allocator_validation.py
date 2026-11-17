"""Source-only acceptance checks for the isolated allocator exercise.

This module reads repository manifests and Rust source; it does not invoke Cargo.
"""
from __future__ import annotations

import json
from pathlib import Path
import re
import tomllib


HARNESS = Path("validation/oa-alloc-check")
FIXTURE_IDS = {
    "tracker_update_ooo_refusal_capacity_advance_flush",
    "fusion_adsb_hard_assoc_registration",
    "fusion_anonymous_init_and_assoc",
}
TRACKER_FIELDS = {
    "applied": 4, "refiltered": 1, "refused": 1, "capacity_refused": 1,
    "predictions": 1, "advance_rows": 1, "flush_rows": 1, "ended": 1, "rows": 1,
}
FUSION_FIELDS = {
    "plots": 40, "hard_applied": 1, "candidates_confirmed": 2, "refused": 1,
    "rows": 1, "radar_tracks": 2, "registration_used": 1,
    "registration_history": 1, "anon_applied": 2, "local_rows": 2,
    "init_visits": 1, "init_tests": 1, "coast_rows": 1, "quiet_advance_rows": 1,
}


def _inside(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _read_toml(path: Path) -> tuple[dict, str | None]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8")), None
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        return {}, f"cannot read valid TOML at {path}: {exc}"


def _manifest_path(manifest: Path, raw: str, root: Path) -> tuple[Path | None, str | None]:
    candidate = (manifest.parent / raw).resolve()
    if not _inside(candidate, root):
        return None, f"path dependency escapes repository: {manifest}: {raw}"
    target = (candidate / "Cargo.toml").resolve()
    if not _inside(target, root):
        return None, f"path dependency manifest escapes repository: {manifest}: {raw}"
    return target, None


def _member_manifests(root: Path, workspace: dict) -> tuple[list[Path], list[str]]:
    errors: list[str] = []
    members: set[Path] = set()
    patterns = workspace.get("members", [])
    if not isinstance(patterns, list):
        return [], ["workspace members must be an array of literal paths"]
    for pattern in patterns:
        if not isinstance(pattern, str):
            errors.append("workspace member entries must be strings")
            continue
        member_path = Path(pattern)
        if member_path.is_absolute() or ".." in member_path.parts or any(c in pattern for c in "*?[]"):
            errors.append(f"workspace member must be a repository-relative literal path: {pattern}")
            continue
        candidate = root / member_path
        resolved_candidate = candidate.resolve()
        if not _inside(resolved_candidate, root):
            errors.append(f"workspace member path escapes repository: {pattern}")
            continue
        if resolved_candidate.is_dir():
            manifest = resolved_candidate / "Cargo.toml"
        elif resolved_candidate.name == "Cargo.toml":
            manifest = resolved_candidate
        else:
            errors.append(f"workspace member must name a crate directory: {pattern}")
            continue
        resolved = manifest.resolve()
        if not _inside(resolved, root):
            errors.append(f"workspace member manifest escapes repository: {pattern}")
            continue
        members.add(resolved)
    return sorted(members), errors


def _all_deps(manifest: dict):
    """Yield dependency declarations, including target-specific tables."""
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        values = manifest.get(section, {})
        if isinstance(values, dict):
            yield from values.items()
    workspace = manifest.get("workspace", {})
    if isinstance(workspace, dict) and isinstance(workspace.get("dependencies"), dict):
        yield from workspace["dependencies"].items()
    targets = manifest.get("target", {})
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                values = target.get(section, {})
                if isinstance(values, dict):
                    yield from values.items()


def _local_dependency_graph(start: list[Path], root: Path, errors: list[str]) -> set[Path]:
    seen: set[Path] = set()
    pending = list(start)
    while pending:
        manifest_path = pending.pop()
        if manifest_path in seen:
            continue
        if not _inside(manifest_path, root):
            errors.append(f"manifest escapes repository: {manifest_path}")
            continue
        seen.add(manifest_path)
        data, problem = _read_toml(manifest_path)
        if problem:
            errors.append(problem)
            continue
        for alias, declaration in _all_deps(data):
            if not isinstance(declaration, dict) or "path" not in declaration:
                continue
            raw = declaration.get("path")
            if not isinstance(raw, str):
                errors.append(f"invalid path dependency in {manifest_path}: {alias}")
                continue
            target, problem = _manifest_path(manifest_path, raw, root)
            if problem:
                errors.append(problem)
            elif target is not None:
                pending.append(target)
    return seen


def _check_dep_backedges(manifests: set[Path], harness_dir: Path, root: Path,
                         errors: list[str]) -> None:
    for manifest_path in manifests:
        data, problem = _read_toml(manifest_path)
        if problem:
            errors.append(problem)
            continue
        for alias, declaration in _all_deps(data):
            package = declaration.get("package") if isinstance(declaration, dict) else None
            if alias == "oa-alloc-check" or package == "oa-alloc-check":
                errors.append(f"product dependency names allocator harness: {manifest_path} ({alias})")
            if not isinstance(declaration, dict) or "path" not in declaration:
                continue
            raw = declaration.get("path")
            if not isinstance(raw, str):
                continue
            target, problem = _manifest_path(manifest_path, raw, root)
            if problem:
                errors.append(problem)
            elif target is not None and _inside(target.parent, harness_dir):
                errors.append(f"product dependency points back to allocator harness: {manifest_path} ({alias})")


_RAW_STRING_START = re.compile(r'(?:br|cr|r)(#*)"')


def _rust_tokens(source: str) -> str:
    """Discard comments and string/char contents before token-level source checks."""
    # A small lexical scrubber avoids false hits in comments/strings while still
    # being deliberately fail-closed when braces or quotes are malformed.
    out: list[str] = []
    i = 0
    state = "code"
    depth = 0
    while i < len(source):
        if state == "code":
            if source.startswith("//", i):
                state = "line"
                out.extend("  ")
                i += 2
            elif source.startswith("/*", i):
                state, depth = "block", 1
                out.extend("  ")
                i += 2
            elif (raw := _RAW_STRING_START.match(source, i)) is not None:
                closing = '"' + raw.group(1)
                end = source.find(closing, raw.end())
                if end < 0:
                    raise ValueError("unterminated Rust raw string")
                end += len(closing)
                out.extend("\n" if char == "\n" else " " for char in source[i:end])
                i = end
            elif source[i] == '"':
                quote = source[i]
                state = "string"
                out.append(" ")
                i += 1
            elif source[i] == "'":
                # Keep Rust lifetimes such as 'static as tokens; blank only a
                # character literal whose closing quote is present.
                end = i + 1
                if end < len(source) and source[end] == "\\":
                    end += 2
                else:
                    end += 1
                if end < len(source) and source[end] == "'":
                    out.extend(" " * (end + 1 - i))
                    i = end + 1
                else:
                    out.append(source[i])
                    i += 1
            else:
                out.append(source[i])
                i += 1
        elif state == "line":
            if source[i] == "\n":
                state = "code"
                out.append("\n")
            else:
                out.append(" ")
            i += 1
        elif state == "block":
            if source.startswith("/*", i):
                depth += 1
                out.extend("  ")
                i += 2
            elif source.startswith("*/", i):
                depth -= 1
                out.extend("  ")
                i += 2
                if depth == 0:
                    state = "code"
            else:
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
        else:
            if source[i] == "\\":
                out.extend("  ")
                i += min(2, len(source) - i)
            elif source[i] == quote:
                out.append(" ")
                i += 1
                state = "code"
            else:
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
    if state not in ("code", "line"):
        raise ValueError("unterminated Rust string or comment")
    return "".join(out)


def allocation_layout_errors(repo: Path) -> list[str]:
    """Return source-layout violations for the standalone allocator harness."""
    errors: list[str] = []
    root = Path(repo).resolve()
    root_manifest = (root / "Cargo.toml").resolve()
    if not _inside(root_manifest, root):
        return ["root Cargo.toml escapes repository"]
    workspace, problem = _read_toml(root_manifest)
    if problem:
        return [problem]

    workspace_table = workspace.get("workspace", {})
    if not isinstance(workspace_table, dict):
        return ["root workspace table must be a TOML table"]
    root_lints = workspace_table.get("lints", {})
    if not isinstance(root_lints, dict):
        root_lints = {}
    root_rust_lints = root_lints.get("rust", {})
    if not isinstance(root_rust_lints, dict):
        root_rust_lints = {}
    if root_rust_lints.get("unsafe_code") != "forbid":
        errors.append("root workspace rust unsafe_code lint must remain forbid")

    members, member_errors = _member_manifests(root, workspace.get("workspace", {}))
    errors.extend(member_errors)
    for member in members:
        if not _inside(member, root):
            errors.append(f"workspace member manifest escapes repository: {member}")
            continue
        member_data, member_problem = _read_toml(member)
        if member_problem:
            errors.append(member_problem)
        elif member_data.get("lints", {}).get("workspace") is not True:
            errors.append(f"product workspace member must inherit workspace lints: {member}")
    harness_dir = (root / HARNESS).resolve()
    if not _inside(harness_dir, root):
        return errors + ["allocator harness path escapes repository"]
    excludes = workspace_table.get("exclude", [])
    if not isinstance(excludes, list) or any(not isinstance(p, str) for p in excludes):
        errors.append("workspace exclude entries must be strings")
        excludes = []
    for pattern in excludes:
        relative = Path(pattern)
        if relative.is_absolute() or ".." in relative.parts or any(c in pattern for c in "*?[]"):
            errors.append(f"workspace exclude must be a repository-relative literal path: {pattern}")
    if HARNESS.as_posix() not in excludes:
        errors.append("allocator harness must be excluded from the product workspace")
    if any(member.parent == harness_dir or _inside(harness_dir, member.parent) for member in members):
        errors.append("allocator harness must not be a product workspace member")

    graph = _local_dependency_graph([root_manifest, *members], root, errors)
    _check_dep_backedges(graph, harness_dir, root, errors)

    manifest_path = (harness_dir / "Cargo.toml").resolve()
    if not _inside(manifest_path, root):
        return errors + ["allocator harness Cargo.toml escapes repository"]
    harness, problem = _read_toml(manifest_path)
    if problem:
        return errors + [problem]
    package = harness.get("package", {})
    if not isinstance(package, dict) or package.get("publish") is not False:
        errors.append("allocator harness package must set publish = false")
    if "workspace" not in harness:
        errors.append("allocator harness must declare its own [workspace]")
    deps = harness.get("dependencies", {})
    if not isinstance(deps, dict) or set(deps) != {"oa-track"}:
        errors.append("allocator harness may declare only the oa-track dependency")
    elif (not isinstance(deps["oa-track"], dict)
          or deps["oa-track"].get("path") != "../../crates/oa-track"
          or set(deps["oa-track"]) != {"path"}):
        errors.append("allocator harness oa-track dependency must be the local crate path")
    for section in ("dev-dependencies", "build-dependencies", "target"):
        if harness.get(section):
            errors.append(f"allocator harness may not declare {section}")

    src_dir = harness_dir / "src"
    src_real = src_dir.resolve()
    if src_dir.exists() and not _inside(src_real, root):
        return errors + ["allocator harness src directory escapes repository"]
    src_sources = sorted(p.name for p in src_real.glob("*.rs")) if src_real.is_dir() else []
    root_sources = sorted(p.name for p in harness_dir.glob("*.rs"))
    if src_sources == ["allocator.rs", "main.rs"]:
        base = src_dir
    elif root_sources == ["allocator.rs", "main.rs"] and not src_dir.exists():
        base = harness_dir
    else:
        errors.append("allocator harness src files must be exactly main.rs and allocator.rs")
        base = src_dir if src_dir.is_dir() else harness_dir
    try:
        if base.is_dir() and any(p.is_symlink() or p.is_dir() for p in base.iterdir()):
            errors.append("allocator harness source directories may not contain nested directories")
    except OSError as exc:
        errors.append(f"cannot inspect allocator harness source directory: {exc}")
    main_path, allocator_path = base / "main.rs", base / "allocator.rs"
    if any(not _inside(p.resolve(), root) for p in (main_path, allocator_path)):
        return errors + ["allocator harness source file escapes repository"]
    if main_path.is_file() and allocator_path.is_file():
        try:
            main = _rust_tokens(main_path.read_text(encoding="utf-8"))
            allocator = _rust_tokens(allocator_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            errors.append(f"cannot read allocator harness source: {exc}")
        else:
            if not re.search(r"#!\s*\[\s*deny\s*\(\s*unsafe_code\s*\)\s*\]", main):
                errors.append("main.rs must begin with #![deny(unsafe_code)]")
            if re.search(r"\bunsafe\b", main):
                errors.append("main.rs may not contain unsafe tokens")
            if re.search(r"#\s*!?\s*\[\s*allow\s*\([^]]*\bunsafe_code\b", main):
                errors.append("main.rs may not allow unsafe_code")
            if not re.search(r"#!\s*\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]", allocator):
                errors.append("allocator.rs must locally allow unsafe_code")
            if len(re.findall(r"#!\s*\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]", allocator)) != 1:
                errors.append("allocator.rs must contain exactly one local unsafe_code allowance")
            all_allows = re.findall(r"#\s*!?\s*\[\s*allow\s*\([^]]*\bunsafe_code\b[^]]*\)\s*\]", allocator)
            if len(all_allows) != 1:
                errors.append("allocator.rs may contain only its module-level unsafe_code allowance")
            if len(re.findall(r"#\s*\[\s*global_allocator\s*\]", allocator)) != 1:
                errors.append("allocator.rs must contain exactly one global allocator attribute")
            if not re.search(r"\bunsafe\s+impl\s+(?:<[^>]*>\s*)?GlobalAlloc\b", allocator):
                errors.append("allocator.rs must contain the unsafe GlobalAlloc implementation")
    return errors


def _nonnegative_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _object(value) -> bool:
    return isinstance(value, dict)


def evaluate_allocator_result(text: str, expect_injected: bool = False) -> list[str]:
    """Validate one JSON result from the bounded M3 allocator harness."""
    errors: list[str] = []
    if not isinstance(expect_injected, bool):
        return ["expect_injected must be a boolean"]
    try:
        result = json.loads(text)
    except (TypeError, ValueError, json.JSONDecodeError) as exc:
        return [f"result is not valid complete JSON: {exc}"]
    if not _object(result):
        return ["result must be a JSON object"]
    required_keys = {"status", "injection_requested", "fixture_ids", "controls", "tracker", "fusion"}
    if set(result) != required_keys:
        errors.append("result must contain exactly the required allocator evidence fields")
    expected_status = "FAIL_M3_ALLOCATION" if expect_injected else "PASS_M3_EXERCISED"
    if result.get("status") != expected_status:
        errors.append(f"status must be {expected_status}")
    if result.get("injection_requested") is not expect_injected:
        errors.append("injection_requested does not match requested mode")

    fixture_ids = result.get("fixture_ids")
    if (not isinstance(fixture_ids, list) or len(fixture_ids) != 3
            or any(not isinstance(item, str) for item in fixture_ids)
            or len(set(fixture_ids)) != 3 or set(fixture_ids) != FIXTURE_IDS):
        errors.append("fixture_ids must contain exactly the three required unique fixtures")

    controls = result.get("controls")
    control_names = {"alloc", "zeroed", "realloc"}
    if not _object(controls) or set(controls) != control_names | {"overflow"}:
        errors.append("controls must contain exactly alloc, zeroed, realloc, and overflow")
        controls = {}
    if controls.get("overflow") is not False:
        errors.append("controls.overflow must be false")
    for name in control_names:
        value = controls.get(name)
        if not _nonnegative_int(value):
            errors.append(f"controls.{name} must be a nonnegative JSON integer")
    if any(controls.get(name) != count for name, count in {"alloc": 2, "zeroed": 1, "realloc": 1}.items()):
        errors.append("controls must report alloc=2, zeroed=1, realloc=1")

    tracker = result.get("tracker")
    if not _object(tracker):
        errors.append("tracker must be an object")
        tracker = {}
    for name in control_names:
        value = tracker.get(name)
        if not _nonnegative_int(value):
            errors.append(f"tracker.{name} must be a nonnegative JSON integer")
        elif not expect_injected and value != 0:
            errors.append(f"normal tracker.{name} must be zero")
        elif expect_injected and name == "alloc" and value < 1:
            errors.append("injected tracker.alloc must be at least one")
    if tracker.get("overflow") is not False:
        errors.append("tracker.overflow must be false")
    for name, minimum in TRACKER_FIELDS.items():
        value = tracker.get(name)
        if not _nonnegative_int(value):
            errors.append(f"tracker.{name} must be a nonnegative JSON integer")
        elif value < minimum:
            errors.append(f"tracker.{name} is below required exercised coverage")

    fusion = result.get("fusion")
    if not _object(fusion):
        errors.append("fusion must be an object")
        fusion = {}
    for name in control_names:
        value = fusion.get(name)
        if not _nonnegative_int(value):
            errors.append(f"fusion.{name} must be a nonnegative JSON integer")
        elif not expect_injected and value != 0:
            errors.append(f"normal fusion.{name} must be zero")
        elif expect_injected and name == "alloc" and value < 1:
            errors.append("injected fusion.alloc must be at least one")
    if fusion.get("overflow") is not False:
        errors.append("fusion.overflow must be false")
    for name, minimum in FUSION_FIELDS.items():
        value = fusion.get(name)
        if not _nonnegative_int(value):
            errors.append(f"fusion.{name} must be a nonnegative JSON integer")
        elif value < minimum:
            errors.append(f"fusion.{name} is below required exercised coverage")
    return errors
