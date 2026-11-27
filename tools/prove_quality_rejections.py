#!/usr/bin/env python3
"""Run bounded synthetic end-to-end refusal proofs for the E05 and E07 class checks."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def _digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=30, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError("bounded proof subprocess failed") from error


def _e05_fixture(repo: Path, private: Path, trace_binary: Path) -> tuple[list[str], Path, dict[str, str]]:
    sys.path.insert(0, str(repo / "tools"))
    import test_check_done_claim as fixtures

    fixture = fixtures.ClaimCheckerTests("test_structural_validation_accepts_complete_synthetic_bindings")
    fixture.setUp()
    try:
        private.mkdir()
        (private / "FAA_ACCEPTANCE_LIST.md").write_bytes(fixture.source_bytes)
        manifest_path = private / "manifest.json"
        manifest_path.write_bytes(fixture.manifest_bytes)
        source_path = "crates/oa-geo/tests/general.rs"
        source_bytes = (repo / source_path).read_bytes()
        symbol, line = "every_source_file_is_unsafe_free_and_lib_forbids_unsafe", 15
        trace_bytes = (
            "| Requirement | Status | Tests |\n|---|---|---|\n"
            f"| OA-GEN-001 | test linked | `{symbol}` ({source_path}:{line}) |\n"
        ).encode()
        trace_rel = f"target/e05-negative-proof-{os.getpid()}/trace.md"
        trace_path = repo / trace_rel
        trace_path.parent.mkdir(parents=True, exist_ok=True)
        trace_path.write_bytes(trace_bytes)
        clause_digests = [
            _digest(fixture.rows["TEST-A"][span["start"] : span["end"]].encode())
            for span in fixture.spans
        ]
        validator_bytes = fixtures.checker._bounded_read(
            trace_binary, fixtures.checker.MAX_TRACE_TOOL_BYTES, "oa-trace validator"
        )
        inventory = {
            **fixture.inventory,
            "source_commit": _run(["git", "rev-parse", "--verify", "HEAD"], cwd=repo).stdout.strip(),
            "tests": [
                {
                    "requirement_id": "OA-GEN-001",
                    "path": source_path,
                    "symbol": symbol,
                    "line": line,
                    "source_sha256": _digest(source_bytes),
                    "clause_sha256": clause_digest,
                }
                for clause_digest in clause_digests
            ],
            "trace_report_path": trace_rel,
            "trace_report_sha256": _digest(trace_bytes),
            "validator_sha256": _digest(validator_bytes),
        }
        inventory_bytes = json.dumps(inventory, sort_keys=True, separators=(",", ":")).encode()
        claim = {
            "schema": fixtures.checker.CLAIM_SCHEMA,
            "source_sha256": fixture.source_digest,
            "manifest_sha256": _digest(fixture.manifest_bytes),
            "test_inventory_sha256": _digest(inventory_bytes),
            "row_id": "TEST-A",
            "clauses": [
                {
                    **span,
                    "status": "covered",
                    "evidence": [{
                        "kind": "test",
                        "requirement_id": "OA-GEN-001",
                        "path": source_path,
                        "line": line,
                        "symbol": symbol,
                        "clause_sha256": clause_digests[index],
                    }],
                }
                for index, span in enumerate(fixture.spans)
            ],
        }
        claim_path = private / "claim.json"
        claim_path.write_text(json.dumps(claim), encoding="utf-8")
        inventory_path = private / "inventory.json"
        inventory_path.write_bytes(inventory_bytes)
        command = [
            sys.executable,
            str(repo / "tools/check_done_claim.py"),
            "--source-kind", "faa", "--repo", str(repo),
            "--manifest", str(manifest_path), "--claim", str(claim_path),
            "--test-inventory", str(inventory_path), "--row", "TEST-A",
            "--source-commit", inventory["source_commit"],
            "--expected-source-sha256", fixture.source_digest,
            "--oa-trace-bin", str(trace_binary),
        ]
        env = os.environ.copy()
        env["OPENATCA_PRIVATE"] = str(private)
        data_root = private.parent / "synthetic-data"
        data_root.mkdir()
        (data_root / ".openatca-data-root").write_bytes(b"openatca-data-root-v1\n")
        env["OPENATCA_DATA"] = str(data_root)
        return command, claim_path, env
    finally:
        fixture.tearDown()


def _prove_e05(repo: Path) -> tuple[str, str]:
    binary = _configured_trace_binary()
    with tempfile.TemporaryDirectory(prefix="oa-e05-negative-proof-") as temporary:
        command, claim_path, env = _e05_fixture(repo, Path(temporary) / "synthetic-private", binary)
        positive = _run(command, cwd=repo, env=env)
        if positive.returncode != 0:
            raise RuntimeError(f"synthetic E05 positive control was refused: {positive.stderr[:384]}")
        claim = json.loads(claim_path.read_text(encoding="utf-8"))
        claim["clauses"][1]["status"] = "external"
        claim_path.write_text(json.dumps(claim), encoding="utf-8")
        refusal = _run(command, cwd=repo, env=env)
        if refusal.returncode != 1 or "not fully covered" not in refusal.stderr:
            raise RuntimeError("E05 external-clause mutant was not refused as expected")
        return _digest(claim_path.read_bytes()), "external-clause-refused"


def _workspace_markers(root: Path) -> Path:
    (root / "docs").mkdir()
    (root / "docs/RULESET.md").write_text("# THE RULESET — OpenATCA synthetic proof\n", encoding="utf-8")
    (root / "crates/oa-track/src").mkdir(parents=True)
    (root / "crates/oa-track/Cargo.toml").write_text("[package]\nname='oa-track'\n", encoding="utf-8")
    (root / "tools/oa-trace").mkdir(parents=True)
    (root / "tools/oa-trace/Cargo.toml").write_text("[package]\nname='oa-trace'\n", encoding="utf-8")
    (root / "tools").mkdir(exist_ok=True)
    (root / "tools/rust_proofs.json").write_text("{}\n", encoding="utf-8")
    (root / "Cargo.toml").write_text(
        "[workspace]\nmembers = [\"crates/oa-track\", \"tools/oa-trace\"]\n", encoding="utf-8"
    )
    source = root / "crates/oa-track/src/lib.rs"
    source.write_text("pub fn baseline() {}\n", encoding="utf-8")
    _run(["git", "init", "-q"], cwd=root)
    _run(["git", "add", "Cargo.toml", "docs", "crates", "tools"], cwd=root)
    return source


def _baseline_for(summary: dict[str, object], ratchet: object) -> dict[str, object]:
    baseline_summary = dict(summary)
    baseline_summary.pop("duplicate_details", None)
    return {
        "schema": 1,
        "algorithm": ratchet.ALGORITHM,
        "scope": ratchet.SCOPE,
        "workspace_binding": ratchet.WORKSPACE_BINDING,
        "limits": {
            "long_function_lines_exceeds": 150,
            "argument_count_exceeds": 7,
            "duplicate_window_tokens": ratchet.WINDOW_TOKENS,
            "duplicate_minimum_distinct_identifiers": ratchet.MIN_IDENTIFIERS,
            "tracked_source_files_max": ratchet.MAX_SOURCE_FILES,
            "tracked_source_bytes_max": ratchet.MAX_SOURCE_BYTES,
            "tracked_source_file_bytes_max": ratchet.MAX_SOURCE_FILE_BYTES,
            "baseline_bytes_max": ratchet.MAX_BASELINE_BYTES,
            "function_body_tokens_max": ratchet.MAX_CODE_TOKENS,
            "duplicate_windows_max": ratchet.MAX_DUPLICATE_WINDOWS,
            "duplicate_block_groups_max": ratchet.MAX_DUPLICATE_BLOCK_GROUPS,
            "duplicate_comparisons_max": ratchet.MAX_DUPLICATE_COMPARISONS,
            "lexer_tokens_per_file_max": ratchet.MAX_LEX_TOKENS_PER_FILE,
        },
        "seed_revision": "0" * 40,
        "summary": baseline_summary,
    }


def _run_ratchet(script: Path, root: Path, baseline: Path) -> subprocess.CompletedProcess[str]:
    return _run(
        [sys.executable, str(script), "--root", str(root), "--baseline", str(baseline), "--check"],
        cwd=root,
    )


def _prove_e07(repo: Path) -> tuple[str, str]:
    sys.path.insert(0, str(repo / "tools"))
    import quality_size_ratchet as ratchet

    with tempfile.TemporaryDirectory(prefix="oa-e07-negative-proof-") as temporary:
        root = Path(temporary)
        source = _workspace_markers(root)
        base_text = source.read_text(encoding="utf-8")
        summary = ratchet.analyze_sources([("crates/oa-track/src/lib.rs", base_text)])
        baseline = root / "tools/quality-size-baseline.json"
        baseline.write_text(json.dumps(_baseline_for(summary, ratchet), sort_keys=True), encoding="utf-8")
        baseline_digest = _digest(baseline.read_bytes())
        baseline.chmod(0o444)
        script = repo / "tools/quality_size_ratchet.py"
        if _run_ratchet(script, root, baseline).returncode != 0:
            raise RuntimeError("synthetic E07 positive control was refused")
        mutations = {
            "long-function": "fn too_long() {\n" + "\n".join("    // retained" for _ in range(150)) + "\n}\n",
            "wide-arguments": "fn too_wide(a,b,c,d,e,f,g,h) {}\n",
            "duplicate-block": _duplicate_functions(),
        }
        for name, bad_text in mutations.items():
            source.write_text(bad_text, encoding="utf-8")
            result = _run_ratchet(script, root, baseline)
            try:
                report = json.loads(result.stdout)
            except json.JSONDecodeError as error:
                raise RuntimeError("E07 CLI did not return its bounded metric report") from error
            if result.returncode != 1 or report.get("status") != "FAIL":
                raise RuntimeError(f"E07 {name} mutant was not refused")
            source.write_text(base_text, encoding="utf-8")
        if _digest(baseline.read_bytes()) != baseline_digest:
            raise RuntimeError("synthetic E07 baseline changed during mutation checks")
        return baseline_digest, "long-function,wide-arguments,duplicate-block-refused"


def _configured_trace_binary() -> Path:
    value = os.environ.get("OPENATCA_OA_TRACE_BIN")
    if not value:
        raise RuntimeError("OPENATCA_OA_TRACE_BIN must name the trusted oa-trace executable")
    binary = Path(value)
    if not binary.is_absolute() or not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("OPENATCA_OA_TRACE_BIN is not an absolute executable file")
    return binary


def _e09_env(root: Path | None) -> dict[str, str]:
    env = {"PATH": os.defpath}
    if root is not None:
        env["OPENATCA_DATA"] = str(root)
    return env


def _expect_e09_refusal(
    command: list[str], cwd: Path, env: dict[str, str], expected: str, label: str, exit_code: int
) -> str:
    result = _run(command, cwd=cwd, env=env)
    if result.returncode != exit_code or expected not in result.stderr:
        raise RuntimeError(
            f"E09 {label} was not refused by the expected sealed-root check: "
            f"exit={result.returncode} stderr={result.stderr[:160]}"
        )
    return label


def _prove_e09(repo: Path) -> tuple[str, str]:
    binary = _configured_trace_binary()
    source = "crates/oa-geo/tests/general.rs"
    test_ref = f"{source}::every_source_file_is_unsafe_free_and_lib_forbids_unsafe"
    script = repo / "tools/sealed_guard.py"
    roots: dict[str, Path] = {}
    with tempfile.TemporaryDirectory(prefix="oa-e09-sealed-root-") as temporary:
        base = Path(temporary)
        valid = base / "valid"
        valid.mkdir()
        (valid / ".openatca-data-root").write_bytes(b"openatca-data-root-v1\n")
        roots["valid"] = valid
        markerless = base / "markerless"
        markerless.mkdir()
        roots["markerless"] = markerless
        wrong = base / "wrong"
        wrong.mkdir()
        (wrong / ".openatca-data-root").write_bytes(b"wrong-root-marker\n")
        roots["wrong"] = wrong
        roots["missing"] = base / "absent"

        py_command = [sys.executable, str(script), source]
        trace_command = [str(binary), "--root", str(repo), "--verify-test", test_ref]
        for command in (py_command, trace_command):
            result = _run(command, cwd=repo, env=_e09_env(valid))
            if result.returncode != 0:
                raise RuntimeError("E09 valid synthetic root did not admit the public source")

        refusals = []
        cases: tuple[tuple[str, Path | None], ...] = (
            ("markerless", roots["markerless"]),
            ("wrong-marker", roots["wrong"]),
            ("missing-root", roots["missing"]),
            ("slash-root", Path("/")),
            ("empty-configuration", None),
        )
        for label, root in cases:
            env = _e09_env(root)
            if label == "empty-configuration":
                env["OPENATCA_DATA"] = ""
            refusals.append(_expect_e09_refusal(
                py_command, repo, env, "refused by OA-SEAL-001", f"python-{label}", 2
            ))
            refusals.append(_expect_e09_refusal(
                trace_command, repo, env, "test source is not a bounded UTF-8 file", f"trace-{label}", 1
            ))
        receipt = json.dumps(refusals, separators=(",", ":")).encode()
        return _digest(receipt), ",".join(refusals)


def _duplicate_functions() -> str:
    lines = []
    for prefix in ("left", "right"):
        body = [f"let common_{name} = source.{name}();" for name in (
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
            "india", "juliet",
        )]
        lines.extend([f"fn {prefix}(source: Source) {{", *body, "}"])
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--class", dest="proof_class", choices=("E05", "E07", "E09"), required=True)
    args = parser.parse_args(argv)
    repo = Path(__file__).resolve().parents[1]
    try:
        if args.proof_class == "E05":
            digest, refused = _prove_e05(repo)
        elif args.proof_class == "E07":
            digest, refused = _prove_e07(repo)
        else:
            digest, refused = _prove_e09(repo)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"proof_setup_error class={args.proof_class} reason={error}", file=sys.stderr)
        return 2
    print(f"proof_fixture=validated class={args.proof_class} refused={refused} fixture_sha256={digest}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
