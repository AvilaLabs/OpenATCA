#!/usr/bin/env python3
"""Run the bounded OpenATCA research candidate gate and retain evidence."""
import argparse
import csv
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time

from allocator_validation import evaluate_allocator_result

ALLOWED_ENV = {"OPENATCA_DATA", "OPENATCA_RAW", "OPENATCA_SNAPSHOT",
               "OPENATCA_GEODTEST", "OPENATCA_CIFP", "OPENATCA_NASR",
               "OPENATCA_ADSB_CLIP", "OPENATCA_SENSORSIM_CLIP"}
LOSS_KEYS = tuple("loss." + k for k in (
    "fusion.plots_refused", "fusion.adsb_address_refused", "fusion.late", "fusion.pending_overflow", "fusion.hold_evicted",
    "fusion.bucket_plot_overflow", "fusion.bucket_work_capped", "fusion.anon_plots_ungated",
    "fusion.init_capped", "fusion.init_plots_refused", "fusion.assoc_refused", "fusion.cand_capacity",
    "fusion.cand_no_height", "fusion.no_height_pairs", "fusion.track_refused",
    "fusion.assign_log_overflow", "fusion.plot_scan_full", "fusion.wide_full",
    "fusion.init_wide_full", "fusion.track_rec_overflow", "fusion.keyinfo_overflow",
    "fusion.scratch_full", "fusion.reg_history_full", "tracker.capacity_refused",
    "tracker.late_window", "tracker.late_ring", "tracker.time_held_discarded",
    "tracker.time_future_dropped", "tracker.time_sensor_evicted", "tracker.rows_unrepresentable",
    "tracker.refused_position_out_of_range", "tracker.refused_bad_position_covariance",
    "tracker.refused_bad_velocity", "tracker.refused_bad_altitude", "tracker.refused_no_address",
    "tracker.refused_time_out_of_range", "out.overflow"))
PATH_TYPES = {
    "OPENATCA_DATA": "dir", "OPENATCA_RAW": "dir", "OPENATCA_SNAPSHOT": "file",
    "OPENATCA_GEODTEST": "file", "OPENATCA_CIFP": "file", "OPENATCA_NASR": "dir",
    "OPENATCA_ADSB_CLIP": "file", "OPENATCA_SENSORSIM_CLIP": "file",
}
SAFE_DATA_CHILDREN = {"raw", "derived", "tools", "fuzz"}


def sha(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def forbidden_path(path):
    return any(part.casefold() in {"scds", "rec"} for part in Path(path).parts)


def resolve_reference_paths(reference, repo):
    """Resolve explicit paths, enforce types, and inspect only DATA's immediate children."""
    resolved = {}
    for key, value in reference.items():
        candidate = Path(value).expanduser()
        if not candidate.is_absolute():
            candidate = repo / candidate
        path = candidate.resolve(strict=True)
        if forbidden_path(path):
            raise ValueError(f"{key} resolves through a forbidden SCDS/REC path")
        expected = PATH_TYPES[key]
        if expected == "dir" and not path.is_dir():
            raise ValueError(f"{key} must resolve to an existing directory")
        if expected == "file" and not path.is_file():
            raise ValueError(f"{key} must resolve to an existing file")
        if key == "OPENATCA_DATA":
            children = list(path.iterdir())
            bad = [p.name for p in children if p.name not in SAFE_DATA_CHILDREN]
            if bad or {p.name for p in children} != SAFE_DATA_CHILDREN:
                raise ValueError("OPENATCA_DATA must contain only raw/derived/tools/fuzz entries")
            roots = []
            for child in children:
                if not child.is_symlink():
                    raise ValueError("OPENATCA_DATA must declare each permitted child as a symlink")
                if forbidden_path(child.name):
                    raise ValueError("OPENATCA_DATA contains a forbidden path component")
                target = child.resolve(strict=True)
                if forbidden_path(target) or not target.is_dir() or target.name != child.name:
                    raise ValueError("OPENATCA_DATA child symlink has an unsafe target")
                roots.append(target)
            if any(a == b or a in b.parents or b in a.parents
                   for i, a in enumerate(roots) for b in roots[i + 1:]):
                raise ValueError("OPENATCA_DATA child roots must be independent")
        resolved[key] = str(path)
    return resolved


def directory_manifest(root, shared_hashes):
    """Hash a deterministic tree, confined to one resolved root, rejecting links/cycles out of it."""
    real_root = Path(root).resolve(strict=True)
    if forbidden_path(real_root) or not real_root.is_dir():
        raise ValueError(f"unsafe or invalid directory root: {real_root}")
    entries, active = [], set()

    def visit(path, rel):
        if forbidden_path(rel) or forbidden_path(path):
            raise ValueError(f"forbidden SCDS/REC component under {real_root}")
        real_dir = path.resolve(strict=True)
        if forbidden_path(real_dir) or not (real_dir == real_root or real_root in real_dir.parents):
            raise ValueError(f"directory symlink escapes permitted root: {path}")
        if not real_dir.is_dir():
            raise ValueError(f"expected directory: {path}")
        if real_dir in active:
            raise ValueError(f"directory cycle at {rel}")
        active.add(real_dir)
        entries.append({"path": rel.as_posix() if rel.parts else ".", "type": "dir"})
        children = sorted(real_dir.iterdir(), key=lambda p: p.name)
        for child in children:
            if forbidden_path(child.name):
                raise ValueError(f"forbidden SCDS/REC component under {real_root}")
            child_rel = rel / child.name
            target = child.resolve(strict=True)
            if forbidden_path(child_rel) or forbidden_path(target):
                raise ValueError(f"forbidden SCDS/REC component under {real_root}")
            if child.is_symlink() and not (target == real_root or real_root in target.parents):
                raise ValueError(f"symlink escapes permitted root: {child_rel}")
            if target.is_dir():
                visit(target, child_rel)
            elif target.is_file():
                canonical = str(target)
                digest = shared_hashes.get(canonical)
                if digest is None:
                    digest = sha(target)
                    shared_hashes[canonical] = digest
                entries.append({"path": child_rel.as_posix(), "type": "file", "sha256": digest})
            else:
                raise ValueError(f"unsupported filesystem entry: {child_rel}")
        active.remove(real_dir)

    visit(real_root, Path())
    entries.sort(key=lambda e: (e["path"], e["type"]))
    encoded = json.dumps(entries, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return {"root": str(real_root), "entries": entries,
            "manifest_sha256": hashlib.sha256(encoded.encode()).hexdigest()}


def safeview_manifest(root, shared_hashes):
    """Hash the four declared roots independently; nested links stay inside their own root."""
    real_root = Path(root).resolve(strict=True)
    if forbidden_path(real_root) or not real_root.is_dir():
        raise ValueError(f"unsafe safe-view root: {real_root}")
    children = sorted(real_root.iterdir(), key=lambda p: p.name)
    if {p.name for p in children} != SAFE_DATA_CHILDREN:
        raise ValueError("safe-view must contain exactly raw/derived/tools/fuzz symlinks")
    roots = {}
    for child in children:
        if forbidden_path(child.name) or not child.is_symlink():
            raise ValueError("safe-view entries must be the four declared safe symlinks")
        target = child.resolve(strict=True)
        if forbidden_path(target) or not target.is_dir() or target.name != child.name:
            raise ValueError(f"unsafe safe-view target for {child.name}")
        roots[child.name] = target
    root_values = list(roots.values())
    if any(a == b or a in b.parents or b in a.parents
           for i, a in enumerate(root_values) for b in root_values[i + 1:]):
        raise ValueError("safe-view targets must be independent roots")
    entries = [{"path": ".", "type": "dir"}]
    for name, target in roots.items():
        child_manifest = directory_manifest(target, shared_hashes)
        for entry in child_manifest["entries"]:
            rel = name if entry["path"] == "." else f"{name}/{entry['path']}"
            enriched = {**entry, "path": rel}
            if entry["path"] == ".":
                enriched["resolved_root"] = str(target)
            entries.append(enriched)
    entries.sort(key=lambda e: (e["path"], e["type"]))
    encoded = json.dumps(entries, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return {"root": str(real_root), "entries": entries,
            "manifest_sha256": hashlib.sha256(encoded.encode()).hexdigest()}


def input_snapshot(resolved, reference_env_path=None, clip_path=None):
    """Snapshot explicit file values and directory manifests with one SHA read per resolved file."""
    shared = {}
    files, directories = {}, {}
    for key, path in resolved.items():
        if PATH_TYPES[key] == "file":
            canonical = Path(path).resolve(strict=True)
            if forbidden_path(canonical) or not canonical.is_file():
                raise ValueError(f"invalid explicit file input: {key}")
            name = str(canonical)
            digest = shared.get(name)
            if digest is None:
                digest = sha(canonical)
                shared[name] = digest
            files[key] = {"path": name, "sha256": digest}
        else:
            directories[key] = (safeview_manifest(path, shared) if key == "OPENATCA_DATA"
                                else directory_manifest(path, shared))
    for key, path in (("--clip", clip_path), ("--reference-env", reference_env_path)):
        if path is not None:
            canonical = Path(path).resolve(strict=True)
            if forbidden_path(canonical) or not canonical.is_file():
                raise ValueError(f"invalid explicit file input: {key}")
            name = str(canonical)
            digest = shared.get(name)
            if digest is None:
                digest = sha(canonical)
                shared[name] = digest
            files[key] = {"path": name, "sha256": digest}
    return {"files": files, "directories": directories}


def source_integrity_changed(start_rev, end_rev, start_status, end_status, start_hashes, end_hashes):
    return (start_rev != end_rev or start_status != end_status or start_hashes != end_hashes)


def evaluate_perf(kv, seconds, expected_s, n, overload=False, require_adsb=True):
    """Return explicit failures; absent evidence never defaults to a pass."""
    errors = []
    required = {"keeps_up", "lost_work", "buffers_stable", "tracks_confirmed",
                "measure_s_asked", "measure_s_done", "truncated",
                "offered_aircraft_unique", "offered_aircraft_expected", "unknown_input_labels"}
    missing = sorted(required - kv.keys())
    if missing:
        errors.append("missing required keys: " + ", ".join(missing))
    losses = {k: kv[k] for k in LOSS_KEYS if k in kv}
    absent_losses = sorted(set(LOSS_KEYS) - kv.keys())
    extra_losses = sorted(k for k in kv if k.startswith("loss.") and k not in LOSS_KEYS)
    if absent_losses:
        errors.append("missing loss counters: " + ", ".join(absent_losses))
    if extra_losses:
        errors.append("unexpected loss counters: " + ", ".join(extra_losses))
    try:
        nums = {k: float(kv[k]) for k in required if k in kv}
        nums.update({k: float(v) for k, v in losses.items()})
        if any(not math.isfinite(v) for v in nums.values()):
            errors.append("non-finite numeric evidence")
    except (TypeError, ValueError):
        nums = {}
        errors.append("invalid numeric evidence")
    if not nums:
        return errors
    positive_losses = [k for k, v in losses.items() if not _finite_zero(v)]
    if any(not _nonnegative_integer(v) for v in losses.values()):
        errors.append("loss counters must be nonnegative integers")
    for key in ("tracks_confirmed", "measure_s_asked", "measure_s_done", "truncated",
                "offered_aircraft_unique", "offered_aircraft_expected", "unknown_input_labels"):
        if key in kv and not _nonnegative_integer(kv[key]):
            errors.append(f"{key} must be a nonnegative integer")
    if nums.get("measure_s_asked") != expected_s or nums.get("measure_s_done") != expected_s or nums.get("truncated") != 0:
        errors.append("measurement window is partial or mismatched")
    if nums.get("buffers_stable") != 1:
        errors.append("buffers are not confirmed stable")
    if nums.get("offered_aircraft_expected") != n or nums.get("offered_aircraft_unique") != n:
        errors.append("measured input aircraft coverage differs from configured n")
    if nums.get("unknown_input_labels") != 0:
        errors.append("measured input has unknown truth labels")
    if overload:
        if nums.get("keeps_up") != 0 or nums.get("lost_work") != 1:
            errors.append("overload must fail keeps_up and report lost_work")
        if not positive_losses:
            errors.append("overload has no nonzero loss counter")
    else:
        if nums.get("keeps_up") != 1 or nums.get("lost_work") != 0:
            errors.append("positive run contradicts keeps_up/lost_work")
        if positive_losses:
            errors.append("positive run has loss: " + ", ".join(positive_losses))
    if not seconds:
        errors.append("missing per-second CSV rows")
    elif len(seconds) != expected_s:
        errors.append("per-second CSV row count does not match full window")
    else:
        try:
            if any(not {"second", "wall_ms", "adsb_offered", "plots_offered", "rows_emitted"}.issubset(r) for r in seconds):
                raise KeyError("missing per-second columns")
            indices = [int(r["second"]) for r in seconds]
            if len(set(indices)) != len(indices) or any(b != a + 1 for a, b in zip(indices, indices[1:])):
                errors.append("per-second indices are duplicate or nonconsecutive")
            vals = [float(r["wall_ms"]) for r in seconds]
            if any(not math.isfinite(x) for x in vals):
                errors.append("non-finite per-second wall_ms")
            elif any(x < 0 for x in vals):
                errors.append("negative per-second wall_ms")
            elif not overload and any(x >= 1000 for x in vals):
                errors.append("per-second CSV contains wall_ms >= 1000")
            cols = {k: [int(r[k]) for r in seconds]
                    for k in ("adsb_offered", "plots_offered", "rows_emitted")}
            if any(v < 0 for xs in cols.values() for v in xs):
                errors.append("per-second input/output counts must be nonnegative")
            if require_adsb and sum(cols["adsb_offered"]) <= 0:
                errors.append("zero ADS-B activity")
            if sum(cols["plots_offered"]) <= 0:
                errors.append("zero plot activity")
            if sum(cols["rows_emitted"]) <= 0:
                errors.append("zero output-row activity")
        except (KeyError, ValueError, TypeError):
            errors.append("invalid or missing per-second CSV evidence")
    return errors


def _finite_zero(v):
    try:
        x = float(v)
        return math.isfinite(x) and x == 0
    except (TypeError, ValueError):
        return False


def _nonnegative_integer(v):
    try:
        n = int(v)
        return n >= 0 and str(n) == str(v).strip()
    except (TypeError, ValueError):
        return False


def evaluate_fusion_csv(text):
    """Check the bounded synthetic M3 smoke rows against frozen study targets."""
    errors, measured = [], []
    expected = {("R|all", "b"): (2, "b  ours, no reg"),
                ("M|all", "b"): (3, "b  ours, no reg"),
                ("M|all", "c"): (3, "c  ours + reg")}
    counts = {}
    sample_fields = ("position_all", "position_straight", "position_turn",
                     "speed_all", "speed_straight", "speed_turn",
                     "heading_all", "heading_straight", "heading_turn")
    used_sample_rows = set()
    try:
        rows = list(csv.reader(io.StringIO(text)))
        header = rows[0]
        if len(header) < 10:
            return ["fusion CSV is missing metric columns"], measured
        for index, row in enumerate(rows[1:], 1):
            if not row:
                continue
            if row[0] not in ("R|all", "M|all"):
                continue
            if len(row) < 2:
                errors.append(f"{row[0]} eligible row is missing its variant label")
                continue
            if not row[1].startswith(("b", "c")):
                continue
            prefix = row[1][0]
            group = (row[0], prefix)
            counts[group] = counts.get(group, 0) + 1
            if group not in expected:
                errors.append(f"unexpected eligible fusion group {group}")
            elif row[1] != expected[group][1]:
                errors.append(f"unexpected eligible variant label {row[1]!r} in {row[0]}")
            if len(row) < 10:
                errors.append(f"{row[0]} {row[1]} is a partial eligible row")
                continue
            try:
                n = int(row[2])
                vals = [float(row[i]) for i in range(3, 10)]
            except (ValueError, TypeError) as e:
                errors.append(f"{row[0]} {row[1]} has malformed eligible values: {e}")
                continue
            if n <= 0 or any(not math.isfinite(v) or v < 0 for v in vals):
                errors.append(f"{row[0]} {row[1]} has empty or negative/nonfinite evidence")
                continue
            sample_counts = None
            sample_row = rows[index + 1] if index + 1 < len(rows) else []
            if (len(sample_row) != 11 or sample_row[0] != row[0] + "|samples"
                    or sample_row[1] != row[1]):
                errors.append(f"{row[0]} {row[1]} is missing its metric class sample counts")
            else:
                used_sample_rows.add(index + 1)
                if not all(_nonnegative_integer(v) for v in sample_row[2:]):
                    errors.append(f"{row[0]} {row[1]} has malformed class sample counts")
                else:
                    ns = [int(v) for v in sample_row[2:]]
                    sample_counts = dict(zip(sample_fields, ns))
                    if any(v == 0 for v in ns):
                        errors.append(f"{row[0]} {row[1]} has empty metric class sample counts")
                    if ns[0] != n or any(ns[j + 1] + ns[j + 2] > ns[j]
                                         for j in (0, 3, 6)):
                        errors.append(f"{row[0]} {row[1]} has inconsistent class sample counts")
            pos_all, _pos_straight, _pos_turn, speed_straight, speed_turn, heading_straight, heading_turn = vals
            limits = (pos_all <= 210, speed_straight <= 4, speed_turn <= 8,
                      heading_straight <= 10, heading_turn <= 25)
            failed = [name for name, ok in zip(
                ("pos_all", "speed_straight", "speed_turn", "heading_straight",
                 "heading_turn"), limits) if not ok]
            measured.append({"section": row[0], "variant": row[1], "n": n,
                             "metrics": vals, "sample_counts": sample_counts,
                             "missed_targets": failed})
            if failed:
                errors.append(f"{row[0]} {row[1]} misses {', '.join(failed)}: {vals}")
        for index, row in enumerate(rows[1:], 1):
            if (len(row) >= 2 and row[0] in ("R|all|samples", "M|all|samples")
                    and row[1].startswith(("b", "c")) and index not in used_sample_rows):
                errors.append(f"orphan or duplicate metric class sample counts at row {index + 1}")
    except (IndexError, StopIteration, ValueError, TypeError, csv.Error) as e:
        errors.append(f"invalid fusion CSV: {e}")
    for group, (want, _label) in expected.items():
        got = counts.get(group, 0)
        if got != want:
            errors.append(f"{group[0]} {group[1]} rows: expected {want}, found {got}")
    if not measured:
        errors.append("no R|all/M|all base rows with scored variants")
    return errors, measured


def evaluate_allocator_report(report, revision):
    """Require complete, source-bound results from the isolated M3 allocator check."""
    if not isinstance(report, dict):
        return ["allocator report is not an object"]
    errors = []
    if report.get("status") != "PASS" or report.get("errors") != [] or report.get("layout_errors") != []:
        errors.append("allocator report did not pass every required check")
    if report.get("binary_unchanged") is not True or report.get("source_unchanged") is not True:
        errors.append("allocator report lacks unchanged source/binary evidence")
    binary_hash = report.get("binary_sha256")
    if not isinstance(binary_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", binary_hash):
        errors.append("allocator binary fingerprint is missing or malformed")
    before, after = report.get("source_before"), report.get("source_after")
    if not isinstance(before, dict) or not isinstance(after, dict):
        errors.append("allocator source snapshots are missing")
    else:
        if before != after or before.get("revision") != revision or before.get("status") != "":
            errors.append("allocator check did not use the frozen clean candidate")
        files = before.get("files")
        if (not isinstance(files, dict) or not files
                or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value)
                       for value in files.values())):
            errors.append("allocator source fingerprints are missing or malformed")
    required = {"format", "lint", "build", "tests", "normal", "injected"}
    commands = report.get("commands")
    seen = set()
    if not isinstance(commands, list):
        errors.append("allocator commands are missing")
        commands = []
    for command in commands:
        if not isinstance(command, dict) or not isinstance(command.get("name"), str):
            errors.append("allocator command evidence is malformed")
            continue
        name = command["name"]
        if name not in required or name in seen:
            errors.append(f"unexpected or duplicate allocator command: {name}")
        seen.add(name)
        code = command.get("exit_code")
        if (command.get("status") != "PASS" or type(code) is not int
                or code != (1 if name == "injected" else 0)):
            errors.append(f"allocator command did not produce its required result: {name}")
    if seen != required:
        errors.append("allocator command set is incomplete")
    hashes = report.get("evidence_hashes")
    expected_logs = {f"{name}.{stream}.txt" for name in required for stream in ("stdout", "stderr")}
    if (not isinstance(hashes, dict) or set(hashes) != expected_logs
            or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value)
                   for value in hashes.values())):
        errors.append("allocator command log fingerprints are incomplete or malformed")
    for name, injected in (("normal", False), ("injected", True)):
        errors.extend(f"allocator {name}: {e}" for e in evaluate_allocator_result(
            json.dumps(report.get(name + "_result")), expect_injected=injected))
    return errors


def read_kv(path):
    result = {}
    for line in path.read_text().splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            result[k.strip()] = v.strip()
    return result


def evaluate_clip_artifacts(directory, stdout):
    errors = []
    if "clip latency skipped" in stdout.lower():
        errors.append("tool reported that clip latency was skipped")
    kv_path = directory / "clip.kv"
    for name in ("clip.kv", "clip.txt", "summary.txt", "results.kv"):
        path = directory / name
        if not path.is_file() or path.stat().st_size == 0:
            errors.append(f"missing or empty {name}")
    if not kv_path.is_file():
        return errors
    kv = read_kv(kv_path)
    try:
        clip_s = float(kv.get("clip_s", "nan"))
        if not math.isfinite(clip_s) or clip_s < 600:
            errors.append("clip measurement is shorter than 600 seconds")
        if int(kv.get("aircraft", "0")) <= 0 or int(kv.get("adsb_reports", "0")) <= 0:
            errors.append("clip has no measured aircraft or ADS-B reports")
        inputs = [int(v) for k, v in kv.items() if re.fullmatch(r"pass\d+\.inputs", k)]
        calls = [int(v) for k, v in kv.items() if re.fullmatch(r"pass\d+\.calls", k)]
        if not inputs or sum(inputs) <= 0 or not calls or sum(calls) <= 0:
            errors.append("clip has no measured pass inputs/calls")
        if not any(int(v) > 0 for k, v in kv.items() if re.fullmatch(r"plots_radar\d+", k)):
            errors.append("clip has no measured radar plots")
    except (ValueError, TypeError):
        errors.append("clip metrics are invalid")
    return errors


def run_gate(args):
    repo = args.repo.resolve()
    out = args.out.resolve()
    target = args.target_dir.resolve()
    if out.exists():
        raise ValueError(f"output directory already exists: {out}")
    if not (repo / "Cargo.toml").is_file():
        raise ValueError("--repo must name the workspace root")
    if out == repo or repo in out.parents:
        raise ValueError("--out must be outside the repository")
    rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, check=True,
                         text=True, capture_output=True).stdout.strip()
    start_status = subprocess.run(["git", "status", "--porcelain"], cwd=repo, check=True,
                                  text=True, capture_output=True).stdout
    if start_status.strip():
        dirty = start_status
        raise ValueError("refusing dirty source tree:\n" + dirty)
    reference = {}
    reference_env_path = None
    reference_env_hash_start = None
    if args.reference_env:
        reference_env_path = args.reference_env.expanduser().resolve(strict=True)
        if any(part.casefold() in {"scds", "rec"} for part in reference_env_path.parts):
            raise ValueError("reference-env resolves through a forbidden SCDS/REC path")
        if not reference_env_path.is_file():
            raise ValueError("reference-env must resolve to a file")
        reference_env_hash_start = sha(reference_env_path)
        reference = json.loads(reference_env_path.read_text())
        if not isinstance(reference, dict) or set(reference) - ALLOWED_ENV:
            raise ValueError("reference-env contains an unsupported OPENATCA key")
        if any(not isinstance(v, str) for v in reference.values()):
            raise ValueError("reference environment values must be strings")
    reference_original = dict(reference)
    reference = resolve_reference_paths(reference, repo)
    clip_path = None
    clip_hash_start = None
    if args.clip:
        clip_path = args.clip.expanduser().resolve(strict=True)
        if any(part.casefold() in {"scds", "rec"} for part in clip_path.parts):
            raise ValueError("--clip resolves through a forbidden SCDS/REC path")
        if not clip_path.is_file():
            raise ValueError(f"clip does not resolve to a file: {clip_path}")
    if not args.revision or rev != args.revision:
        raise ValueError("--revision must be the full resolved HEAD hash")
    if target == repo or repo in target.parents:
        raise ValueError("--target-dir must be outside the repository")
    lock = repo / "Cargo.lock"
    lock_text = lock.read_text()
    if 'source = "registry+' in lock_text or 'source = "git+' in lock_text:
        raise ValueError("Cargo.lock contains an external registry or git dependency")

    out.mkdir(parents=True)
    (out / "commands").mkdir()
    records = []
    failures = []
    env = {k: v for k, v in os.environ.items() if not k.startswith("OPENATCA_")}
    env.update(reference)
    env["OPENATCA_SOURCE_REV"] = rev
    env["CARGO_BUILD_JOBS"] = "1"
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO_TARGET_DIR"] = str(target)
    env["RUSTFLAGS"] = ""

    def command(name, argv, timeout=3600, check=True):
        started = time.time()
        try:
            p = subprocess.run(argv, cwd=repo, env=env, text=True, capture_output=True,
                               timeout=timeout)
            rc, stdout, stderr = p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired as e:
            rc = 124
            stdout = (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
            stderr = (e.stderr or b"TIMEOUT").decode(errors="replace") if isinstance(e.stderr, bytes) else (e.stderr or "TIMEOUT")
        stem = out / "commands" / name
        stem.with_suffix(".stdout.txt").write_text(stdout)
        stem.with_suffix(".stderr.txt").write_text(stderr)
        rec = {"name": name, "argv": argv, "exit_code": rc,
               "status": ("NOT_RUN" if name.startswith(("perf_", "synthetic_")) and rc == 0
                          else "PASS" if rc == 0 else "FAIL"),
               "elapsed_s": round(time.time() - started, 3),
               "stdout_sha256": sha(stem.with_suffix(".stdout.txt")),
               "stderr_sha256": sha(stem.with_suffix(".stderr.txt"))}
        records.append(rec)
        if check and rc != 0:
            failures.append(f"{name}: exit {rc}")
        return rc, stdout

    def cargo(name, *argv, timeout=3600):
        return command(name, ["cargo", "+1.89.0", *argv], timeout)[0] == 0

    source_hashes, start_inputs, end_inputs = {}, {}, {}
    start_head = rev
    gate_error = None
    try:
        # Tracked candidate source is hashed before execution; external data is separate.
        paths = subprocess.run(["git", "ls-files", "-z"], cwd=repo, check=True,
                               capture_output=True).stdout.split(b"\0")
        source_hashes = {Path(p.decode()).as_posix(): sha(repo / p.decode()) for p in paths if p}
        start_inputs = input_snapshot(reference, reference_env_path, clip_path)
        if reference_env_path:
            snap_hash = start_inputs["files"].get("--reference-env", {}).get("sha256")
            if snap_hash != reference_env_hash_start:
                raise ValueError("reference-env changed while loading its configuration")
        clip_hash_start = start_inputs["files"].get("--clip", {}).get("sha256")
        build_ok = cargo("build_release", "build", "--locked", "--release", "-p", "oa-bench",
                         "-p", "oa-track", "-p", "oa-perf")
        if build_ok:
            cargo("fmt", "fmt", "--all", "--", "--check")
            cargo("clippy", "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings")
            cargo("workspace_tests", "test", "--locked", "--release", "--workspace")
            trace_tests = cargo("trace_tests", "test", "--locked", "-p", "oa-trace")
            if trace_tests:
                rc, trace = command("trace_run", ["cargo", "+1.89.0", "run", "--locked",
                                                   "-p", "oa-trace", "--", "--root", str(repo)], 600)
                if rc == 0 and "PASS" not in trace:
                    failures.append("trace_run: exit succeeded without PASS evidence")
                    records[-1]["status"] = "FAIL"
            else:
                records.append({"name": "trace_run", "status": "NOT_RUN",
                                "reason": "oa-trace tests failed"})
            allocator_out = out / "allocator"
            allocator_target = target / "allocator-check"
            allocator_rc, _ = command("allocator_check", [sys.executable,
                str(repo / "tools/check_allocations.py"), "--repo", str(repo),
                "--target-dir", str(allocator_target), "--out", str(allocator_out)])
            allocator_errors = []
            try:
                allocator_report = json.loads((allocator_out / "result.json").read_text())
                allocator_errors.extend(evaluate_allocator_report(allocator_report, rev))
                if allocator_report.get("source_before", {}).get("files") != source_hashes:
                    allocator_errors.append("allocator source hashes differ from candidate source hashes")
                binary = allocator_target / "release/oa-alloc-check"
                if not binary.is_file() or sha(binary) != allocator_report.get("binary_sha256"):
                    allocator_errors.append("allocator binary fingerprint does not match the executed binary")
                for name, digest in allocator_report.get("evidence_hashes", {}).items():
                    if (Path(name).name != name or not (allocator_out / name).is_file()
                            or sha(allocator_out / name) != digest):
                        allocator_errors.append(f"allocator evidence fingerprint mismatch: {name}")
                for name in ("normal", "injected"):
                    actual = json.loads((allocator_out / f"{name}.stdout.txt").read_text())
                    if actual != allocator_report.get(name + "_result"):
                        allocator_errors.append(f"allocator {name} payload differs from its command output")
            except (OSError, ValueError, TypeError, AttributeError) as error:
                allocator_errors.append(f"allocator result unavailable: {error}")
            records[-1]["status"] = "FAIL" if allocator_rc != 0 or allocator_errors else "PASS"
            records.append({"name": "evaluate_allocator", "status": records[-1]["status"],
                            "errors": allocator_errors,
                            "claim_scope": "named M3 fixtures, not all inputs or all real-time crates"})
            failures.extend("allocator: " + error for error in allocator_errors)
            for n, tag, warm, frac in ((1200, "stress_1200", 60, "0.5"),
                                       (2400, "stress_2400", 60, "0.5"),
                                       (1200, "radar_only_1200", 120, "0")):
                perf_dir = out / tag
                duration = 60
                argsv = ["one", "--out", str(perf_dir), "--n", str(n), "--layout", "stress",
                         "--radars", "e,t,t", "--adsb-frac", frac, "--max-tracks", "65536",
                         "--seed", "41", "--warmup", str(warm), "--duration", str(duration),
                         "--max-wall", "180", "--no-early-stop"]
                command_name = "perf_" + tag
                perf_rc, _ = command(command_name, [str(target / "release" / "oa-perf"), *argsv], 1800)
                kvp = perf_dir / f"cap_stress_n{n}.kv"
                csvp = perf_dir / f"seconds_stress_n{n}.csv"
                if kvp.exists() and csvp.exists():
                    with csvp.open(newline="") as f:
                        rows = list(csv.DictReader(f))
                    er = evaluate_perf(read_kv(kvp), rows, duration, n, require_adsb=(frac != "0"))
                    records.append({"name": "evaluate_" + tag,
                                   "status": "FAIL" if er else "PASS", "errors": er})
                    records[-2]["status"] = "FAIL" if perf_rc != 0 or er else "PASS"
                    if er:
                        failures.extend(f"perf_{tag}: {e}" for e in er)
                else:
                    records[-1]["status"] = "FAIL"
                    failures.append(f"perf_{tag}: missing output evidence")
            overload_dir = out / "overload"
            overload_rc, _ = command("perf_overload", [str(target / "release" / "oa-perf"), "one", "--out",
                    str(overload_dir), "--n", "1200", "--layout", "stress", "--radars", "e,t,t",
                    "--adsb-frac", "0.5", "--seed", "41", "--warmup", "60", "--duration", "60",
                    "--max-wall", "180", "--max-tracks", "40", "--no-early-stop"], 1800)
            kvp = overload_dir / "cap_stress_n1200.kv"
            if kvp.exists():
                with (overload_dir / "seconds_stress_n1200.csv").open(newline="") as f:
                    rows = list(csv.DictReader(f))
                er = evaluate_perf(read_kv(kvp), rows, 60, 1200, overload=True)
                records.append({"name": "evaluate_overload",
                               "status": "FAIL" if er else "PASS", "errors": er})
                records[-2]["status"] = "FAIL" if overload_rc != 0 or er else "PASS"
                if er:
                    failures.extend(f"perf_overload: {e}" for e in er)
            else:
                records[-1]["status"] = "FAIL"
                failures.append("perf_overload: missing output evidence")
            # Bounded M3 smoke: synthetic target check, not an ESASSP compliance claim.
            fusion_out = out / "fusion_base"
            rc, _ = command("fusion_base", [str(target / "release" / "oa-track-fusion"),
                        "--smoke", "--only", "base", "--out", str(fusion_out)], 1200)
            fusion_csv = fusion_out / "tables.csv"
            if rc == 0 and fusion_csv.is_file():
                fusion_errors, fusion_rows = evaluate_fusion_csv(fusion_csv.read_text())
                records.append({"name": "evaluate_fusion_base",
                                "status": "FAIL" if fusion_errors else "PASS",
                                "errors": fusion_errors, "rows": fusion_rows,
                                "claim_scope": "bounded synthetic target check; not ESASSP compliance"})
                records[-2]["status"] = "FAIL" if rc != 0 or fusion_errors else "PASS"
                failures.extend("fusion_base: " + e for e in fusion_errors)
            else:
                records.append({"name": "evaluate_fusion_base", "status": "NOT_RUN",
                                "reason": "command failed or tables.csv missing"})
                if rc == 0:
                    records[-2]["status"] = "FAIL"
                    failures.append("fusion_base: missing tables.csv")
            # Synthetic M2 self-test twice, with byte-identical reports and all checks passing.
            for i in (1, 2):
                d = out / f"synthetic_{i}"
                synth_rc, out_text = command(f"synthetic_{i}", [str(target / "release" / "oa-bench"),
                        "--out", str(d), "--synthetic"], 1200)
                match = re.search(r"(\d+) checks, 0 failed", out_text)
                report_path = d / "report.txt"
                check_errors = []
                if not match or int(match.group(1)) == 0:
                    check_errors.append("stdout did not report nonempty checks with zero failures")
                if not report_path.is_file():
                    check_errors.append("report.txt missing")
                else:
                    report_text = report_path.read_text()
                    ok_notes = re.findall(r"^note: check .*: ok$", report_text, re.MULTILINE)
                    if not match or len(ok_notes) != int(match.group(1)):
                        check_errors.append("report notes do not match the check count")
                    if "FAILED" in report_text:
                        check_errors.append("report contains a failed check")
                    if f"source rev {rev}" not in report_text:
                        check_errors.append("report source revision does not match full HEAD")
                records.append({"name": f"evaluate_synthetic_{i}",
                                "status": "FAIL" if check_errors else "PASS", "errors": check_errors})
                records[-2]["status"] = "FAIL" if synth_rc != 0 or check_errors else "PASS"
                failures.extend(f"synthetic_{i}: {e}" for e in check_errors)
            d1, d2 = out / "synthetic_1", out / "synthetic_2"
            synthetic_errors = []
            for name in ("report.csv", "report.txt"):
                p1, p2 = d1 / name, d2 / name
                if not p1.is_file() or not p2.is_file():
                    synthetic_errors.append(f"missing {name}")
                elif sha(p1) != sha(p2):
                    synthetic_errors.append(f"repeat {name} differs")
            records.append({"name": "evaluate_synthetic_repeats",
                            "status": "FAIL" if synthetic_errors else "PASS",
                            "errors": synthetic_errors})
            if synthetic_errors:
                for rec in records:
                    if rec.get("name") in ("synthetic_1", "synthetic_2"):
                        rec["status"] = "FAIL"
            failures.extend("synthetic: " + e for e in synthetic_errors)
            if args.clip:
                rc, clip_stdout = command("perf_clip", [str(target / "release" / "oa-perf"), "clip",
                        "--clip", str(clip_path), "--clip-secs", "600", "--out",
                        str(out / "clip")], 1800)
                clip_errors = evaluate_clip_artifacts(out / "clip", clip_stdout)
                records.append({"name": "evaluate_perf_clip",
                                "status": "FAIL" if rc != 0 or clip_errors else "PASS",
                                "errors": clip_errors})
                records[-2]["status"] = "FAIL" if rc != 0 or clip_errors else "PASS"
                failures.extend("perf_clip: " + e for e in clip_errors)
            else:
                records.append({"name": "perf_clip", "status": "NOT_RUN", "reason": "no --clip"})
    except Exception as e:
        gate_error = f"{type(e).__name__}: {e}"
        failures.append(f"runner exception: {gate_error}")
        records.append({"name": "runner", "status": "FAIL",
                        "error": gate_error})
    finally:
        required_names = ["fmt", "clippy", "workspace_tests", "trace_tests", "trace_run",
                          "perf_stress_1200", "perf_stress_2400", "perf_radar_only_1200",
                          "perf_overload", "fusion_base", "synthetic_1", "synthetic_2", "allocator_check", "evaluate_allocator"]
        present = {r.get("name") for r in records}
        records.extend({"name": name, "status": "NOT_RUN", "reason": "release build failed"}
                       for name in required_names if name not in present)
        binary_hashes = {}
        binaries = {
            "oa-bench": target / "release" / "oa-bench",
            "oa-perf": target / "release" / "oa-perf",
            "oa-track-run": target / "release" / "oa-track-run",
            "oa-track-fusion": target / "release" / "oa-track-fusion",
            "oa-trace": target / "debug" / "oa-trace",
            "oa-alloc-check": target / "allocator-check/release/oa-alloc-check",
        }
        for name, p in binaries.items():
            if p.is_file():
                binary_hashes[name] = sha(p)
        open_claims = [
            {"claim": "allocation C5", "status": "OPEN",
             "reason": "named M3 routes checked separately; all-input and other real-time-core coverage unproved"},
            {"claim": "matched ERAM comparison", "status": "OPEN", "reason": "not run"},
            {"claim": "untouched holdout / independent truth", "status": "OPEN",
             "reason": "not established"},
        ]
        integrity_errors = []
        try:
            end_rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, check=True,
                                     text=True, capture_output=True).stdout.strip()
            end_status = subprocess.run(["git", "status", "--porcelain"], cwd=repo, check=True,
                                        text=True, capture_output=True).stdout
            end_paths = subprocess.run(["git", "ls-files", "-z"], cwd=repo, check=True,
                                       capture_output=True).stdout.split(b"\0")
            end_hashes = {Path(p.decode()).as_posix(): sha(repo / p.decode()) for p in end_paths if p}
            end_inputs = input_snapshot(reference, reference_env_path, clip_path)
            if source_integrity_changed(start_head, end_rev, start_status, end_status,
                                        source_hashes, end_hashes):
                integrity_errors.append("source tree or tracked source hashes changed during validation")
            if end_inputs != start_inputs:
                integrity_errors.append("explicit reference input files changed during validation")
        except Exception as e:
            gate_error = gate_error or f"integrity {type(e).__name__}: {e}"
            integrity_errors.append(gate_error)
            failures.append(f"integrity error: {gate_error}")
        records.append({"name": "integrity", "status": "FAIL" if integrity_errors else "PASS",
                        "errors": integrity_errors})
        failures.extend(integrity_errors)
        report = {"revision": rev, "source_files": source_hashes,
                  "requested_revision": args.revision,
                  "binary_hashes": binary_hashes, "reference_config": reference,
                  "reference_config_original": reference_original,
                  "external_inputs_start": start_inputs,
                  "external_inputs_end": end_inputs,
                  "clip_sha256": clip_hash_start,
                  "reference_env_sha256": reference_env_hash_start,
                  "evidence_hashes": {p.relative_to(out).as_posix(): sha(p)
                                      for p in out.rglob("*") if p.is_file() and p.name != "run.json"},
                  "commands": records, "failures": failures, "open_claims": open_claims,
                  "scope_limits": ["full NAS capacity not claimed"],
                  "gate_error": gate_error,
                  "release_ready": False,
                  "final_status": "FAIL" if failures else "OPEN"}
        (out / "run.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    return 1 if failures else 2


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--repo", type=Path, required=True)
    p.add_argument("--revision", required=True)
    p.add_argument("--target-dir", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--reference-env", type=Path)
    p.add_argument("--clip", type=Path)
    args = p.parse_args()
    try:
        code = run_gate(args)
    except (OSError, ValueError, subprocess.CalledProcessError, json.JSONDecodeError) as e:
        print(f"candidate gate: {e}", file=sys.stderr)
        return 1
    return code


if __name__ == "__main__":
    raise SystemExit(main())
