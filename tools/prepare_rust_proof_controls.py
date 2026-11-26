#!/usr/bin/env python3
"""Provision source-only negative-control checkouts; never invokes Cargo or Kani."""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

DEFAULT_SOURCE = Path(__file__).resolve().parent.parent
DEFAULT_OUT = Path("/tmp/openatca-rust-assurance/negative-controls")


def git(repo: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return proc.stdout.strip()


def git_z(repo: Path, *args: str) -> list[str]:
    proc = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return [x.decode("utf-8") for x in proc.stdout.split(b"\0") if x]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def source_changes(source: Path) -> tuple[list[str], list[str]]:
    changed = git_z(source, "diff", "--name-only", "--diff-filter=ACMRTUXB", "-z", "HEAD")
    deleted = git_z(source, "diff", "--name-only", "--diff-filter=D", "-z", "HEAD")
    untracked = git_z(source, "ls-files", "--others", "--exclude-standard", "-z")
    return sorted(set(changed + untracked)), sorted(set(deleted))


def freeze_overlay(source: Path, root: Path, files: list[str], deleted: list[str]) -> tuple[Path, dict[str, str]]:
    """Capture the dirty candidate exactly once before creating any control checkout."""
    snapshot = root / "source-snapshot" / "frozen"
    snapshot.mkdir(parents=True, exist_ok=True)
    hashes: dict[str, str] = {}
    for rel in files:
        src = source / rel
        snap = snapshot / rel
        if not src.is_file() or src.is_symlink():
            raise RuntimeError(f"changed source is not a regular file: {rel}")
        snap.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, snap)
        hashes[rel] = sha256(snap)
    # Detect edits during capture so all worktrees share one stable byte snapshot.
    files_after, deleted_after = source_changes(source)
    if files_after != files or deleted_after != deleted:
        raise RuntimeError("changed-source path set changed while freezing the snapshot")
    for rel, digest in hashes.items():
        if sha256(source / rel) != digest:
            raise RuntimeError(f"source changed while freezing snapshot: {rel}")
    return snapshot, hashes


def copy_overlay(snapshot: Path, checkout: Path, files: list[str], deleted: list[str]) -> dict[str, str]:
    copied: dict[str, str] = {}
    for rel in files:
        src = snapshot / rel
        dst = checkout / rel
        if not src.is_file():
            raise RuntimeError(f"frozen source is missing: {rel}")
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        copied[rel] = sha256(dst)
        if copied[rel] != sha256(src):
            raise RuntimeError(f"snapshot copy hash mismatch: {rel}")
    for rel in deleted:
        dst = checkout / rel
        if dst.is_file() or dst.is_symlink():
            dst.unlink()
    return copied


def mutate(checkout: Path, rel: str, old: str, new: str) -> tuple[str, str, str]:
    path = checkout / rel
    text = path.read_text(encoding="utf-8")
    if text.count(old) != 1:
        raise RuntimeError(f"expected exactly one mutation anchor in {rel}; found {text.count(old)}")
    updated = text.replace(old, new, 1)
    old_hash = sha256(path)
    path.write_text(updated, encoding="utf-8", newline="")
    new_hash = sha256(path)
    diff = "".join(difflib.unified_diff(
        text.splitlines(keepends=True), updated.splitlines(keepends=True),
        fromfile=f"a/{rel}", tofile=f"b/{rel}",
    ))
    return old_hash, new_hash, diff


def prep_control(
    snapshot: Path,
    frozen_hashes: dict[str, str],
    seed: Path,
    root: Path,
    name: str,
    commit: str,
    files: list[str],
    deleted: list[str],
    harness: dict[str, str],
    mutation: tuple[str, str, str] | None,
) -> dict[str, object]:
    checkout = root / "worktrees" / name
    checkout.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "-C", str(seed), "worktree", "add", "--detach", str(checkout), commit],
        check=True,
    )
    source_hashes = copy_overlay(snapshot, checkout, files, deleted)
    if source_hashes != frozen_hashes:
        raise RuntimeError(f"overlay differs from frozen source snapshot for {name}")
    patch_record: dict[str, str] | None = None
    patch_text = ""
    if mutation is not None:
        rel, old, new = mutation
        before_hash, after_hash, patch_text = mutate(checkout, rel, old, new)
        patch_dir = root / "patches"
        patch_dir.mkdir(parents=True, exist_ok=True)
        (patch_dir / f"{name}.patch").write_text(patch_text, encoding="utf-8")
        patch_record = {"path": rel, "pre_mutation_sha256": before_hash, "post_mutation_sha256": after_hash}
    checkout_namespace = hashlib.sha256(str(checkout.resolve()).encode("utf-8")).hexdigest()
    target_dir = Path("/tmp/openatca-rust-assurance/build-cache/checkouts") / checkout_namespace
    env_file = root / "worktrees" / f"{name}.env"
    env_file.write_text(f'export CARGO_TARGET_DIR="{target_dir}"\n', encoding="utf-8")
    manifest = {
        "control": name,
        "base_commit": commit,
        "checkout": str(checkout),
        "target_dir": str(target_dir),
        "cargo_env_file": str(env_file),
        "overlaid_source_hashes_sha256": source_hashes,
        "deleted_overlays": deleted,
        "patch": patch_record,
        "expected_proof": harness,
        "source_snapshot_dir": str(snapshot),
        "checkout_namespace_sha256": checkout_namespace,
        "builds_or_proofs_run_by_provisioner": False,
    }
    (root / "manifests").mkdir(parents=True, exist_ok=True)
    (root / "manifests" / f"{name}.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    root = args.out.resolve()
    if root.exists() and any(root.iterdir()):
        parser.error(f"refusing to overwrite nonempty output directory: {root}")
    root.mkdir(parents=True, exist_ok=True)
    commit = git(source, "rev-parse", "HEAD")
    files, deleted = source_changes(source)
    snapshot, frozen_hashes = freeze_overlay(source, root, files, deleted)

    seed = root / "git-seed"
    subprocess.run(
        ["git", "clone", "--shared", "--no-checkout", str(source), str(seed)],
        check=True,
    )
    names = ["baseline", "output-column", "resync-emit", "caller-track-restore", "late-rollback", "sensor-oldest-tie", "mono-split"]
    harnesses = {
        "baseline": {"purpose": "same current source overlay without a mutation"},
        "output-column": {
            "package": "oa-track-core",
            "harness": "output_push_appends_every_column_bit_exactly_and_preserves_prefix",
            "expected_assertion": "assert_aligned / column-length invariant (the omitted flags column)",
            "failure_kind": "counterexample to a Kani property assertion",
        },
        "resync-emit": {
            "package": "oa-track-core",
            "harness": "confirmed_applied_or_resynced_attempts_one_current_report_row",
            "expected_assertion": "assert_eq!(out.t_ns.len(), 1) on the Resynced path",
            "failure_kind": "counterexample to a Kani property assertion",
        },
        "caller-track-restore": {
            "package": "oa-track-core",
            "harness": "tracker::proofs::rollback_paths::in_order_refusal_restores_track_metadata_projection",
            "expected_assertion": "assert_eq!(t.first_ns[PROOF_SLOT], first)",
            "failure_kind": "counterexample to a Kani property assertion",
            "configuration": {
                "cfgs": [],
                "stubbing": True,
                "stub_summaries": [
                    "decide independently dirties the selected transaction projection with symbolic values and returns NumericalRefused; production update/capture/restore remain real."
                ],
                "required_covers": ["metadata projection dirtied before refusal"],
                "loop_unwind_overrides": [],
                "harness_timeout_seconds": 300,
                "external_timeout_seconds": 420,
                "memory_mib": 8192,
            },
        },
        "late-rollback": {
            "package": "oa-track-core",
            "harness": "tracker::proofs::rollback_paths::late_overflow_output_projection",
            "expected_assertion": "assert_output_txn_eq(&case.before_output, &output_txn(&case.output))",
            "failure_kind": "counterexample to a Kani property assertion",
            "configuration": {
                "cfgs": [],
                "stubbing": True,
                "stub_summaries": [
                    "filter_apply changes the inserted replay event then returns NumericalRefused for the later event; filter_output returns the late row; unexpected replay/restart paths panic."
                ],
                "required_covers": [],
                "loop_unwind_overrides": ["late-replay-two-events-pinned-cargo-one-loop"],
                "harness_timeout_seconds": 600,
                "external_timeout_seconds": 720,
                "memory_mib": 12288,
            },
        },
        "sensor-oldest-tie": {
            "package": "oa-track-core",
            "harness": "time_gate::proofs_full::full_sensor_table_note_new_id_evicts_first_argmin",
            "expected_assertion": "assert_eq!(g.sensor_id[i], SENSOR_TABLE as u16)",
            "failure_kind": "counterexample to a Kani property assertion",
            "configuration": {
                "cfgs": ["kani_full_capacity"],
                "stubbing": False,
                "stub_summaries": [],
                "required_covers": [
                    "eviction counter saturates",
                    "minimum report-time input",
                    "maximum report-time input",
                    "tied minimum keeps earliest slot",
                ],
                "loop_unwind_overrides": [],
                "harness_timeout_seconds": 600,
                "external_timeout_seconds": 720,
                "memory_mib": 12288,
            },
        },
        "mono-split": {
            "package": "oa-replay",
            "harness": "split_matches_i128_for_the_full_typed_domain",
            "expected_assertion": "assert_eq!(actual, expected)",
            "failure_kind": "counterexample to a Kani property assertion",
        },
    }
    # Each mutation is one source statement/block only. Harnesses, assumptions, and proof files
    # are copied unchanged from the current working tree.
    resync_old = """        Step::Resynced => {
            t.stats.applied += 1;
            emit_if_confirmed(t, slot, out);
        }"""
    resync_new = """        Step::Resynced => {
            t.stats.applied += 1;
        }"""
    rollback_old = """            Step::NumericalRefused => {
                restore_track_txn(t, slot, transaction);
                restore_output_txn(out, output);
                return;
            }"""
    rollback_new = """            Step::NumericalRefused => {
                restore_track_txn(t, slot, transaction);
                return;
            }"""
    caller_track_restore_old = """        Step::NumericalRefused => {
            restore_track_txn(t, slot, snapshot);
            restore_output_txn(out, output);
        }"""
    caller_track_restore_new = """        Step::NumericalRefused => {
            restore_output_txn(out, output);
        }"""
    controls: dict[str, tuple[str, str, str] | None] = {
        "baseline": None,
        "output-column": (
            "crates/oa-track-core/src/types.rs",
            "    o.flags.push(r.flags);\n",
            "",
        ),
        "resync-emit": ("crates/oa-track-core/src/tracker.rs", resync_old, resync_new),
        "caller-track-restore": (
            "crates/oa-track-core/src/tracker.rs",
            caller_track_restore_old,
            caller_track_restore_new,
        ),
        "late-rollback": ("crates/oa-track-core/src/tracker.rs", rollback_old, rollback_new),
        "sensor-oldest-tie": (
            "crates/oa-track-core/src/time_gate.rs",
            "                if ns < oldest_ns {\n",
            "                if ns <= oldest_ns {\n",
        ),
        "mono-split": (
            "crates/oa-replay/src/core/mono_split.rs",
            "    wall_ns\n        .checked_sub(mono_ns)\n        .map(|step_ns| (wall_ns, mono_ns, step_ns))\n",
            "    Some((wall_ns, mono_ns, wall_ns))\n",
        ),
    }
    records = []
    for name in names:
        records.append(prep_control(
            snapshot, frozen_hashes, seed, root, name, commit, files, deleted,
            harnesses[name], controls[name]
        ))
    overview = {
        "source_checkout": str(source),
        "base_commit": commit,
        "output_root": str(root),
        "changed_source_paths": files,
        "deleted_source_paths": deleted,
        "frozen_source_hashes_sha256": frozen_hashes,
        "target_cache_rule": "/tmp/openatca-rust-assurance/build-cache/checkouts/SHA256(resolved_checkout_path)",
        "controls": names,
        "policy": "Provisioning and SHA-256 inventory only; no compiler, Cargo, tests, or Kani invoked.",
    }
    (root / "overview.json").write_text(json.dumps(overview, indent=2, sort_keys=True) + "\n")
    print(json.dumps(overview, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as exc:
        print(f"command failed: {exc.cmd!r} (exit {exc.returncode})", file=sys.stderr)
        raise SystemExit(exc.returncode or 1)
    except Exception as exc:
        print(f"provisioning failed: {exc}", file=sys.stderr)
        raise SystemExit(2)
