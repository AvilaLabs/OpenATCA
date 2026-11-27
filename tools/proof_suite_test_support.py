"""Shared bounded synthetic workspace and runner for proof-suite tests."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
from typing import Any


def proof_entry(overrides: dict[str, Any] | None = None) -> dict[str, Any]:
    """Return one valid row, with only explicit per-test overrides."""
    proof: dict[str, Any] = {
        "id": "sample-proof",
        "requirement": "A bounded arithmetic invariant.",
        "package": "oa-track",
        "harness": "types::proofs::bounded_example",
        "enabled": True,
        "experimental": False,
        "required_covers": ["append reached"],
        "stubbing": False,
        "stub_summaries": [],
        "cfgs": [],
        "harness_timeout_seconds": 300,
        "external_timeout_seconds": 420,
        "memory_mib": 4096,
        "tier": "push",
        "ci_seconds": 10,
        "ci_seconds_source": "ci",
    }
    proof.update(overrides or {})
    return proof


def registry_with(*proofs: dict[str, Any]) -> dict[str, Any]:
    return {"registry_version": 1, "expected_proof_count": len(proofs), "proofs": list(proofs)}


def fake_runner_source() -> str:
    """Generate a validator fixture supporting suite and resource-specific env options."""
    return '''import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
def value(flag): return args[args.index(flag) + 1]
out = pathlib.Path(value("--out"))
out.mkdir(parents=True, exist_ok=True)
package = value("--package")
harness = value("--harness")
covers = [item.split("=", 1)[1] for flag, item in zip(args, args[1:]) if flag == "--require-cover"]
loop_presets = [item for flag, item in zip(args, args[1:]) if flag == "--loop-unwind-preset"]
mode = os.environ.get("FAKE_RUN_RECORD_MODE", "pass")
invocation_log = os.environ.get("FAKE_INVOKED")
if invocation_log:
    try:
        prior_invocations = pathlib.Path(invocation_log).read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        prior_invocations = []
    if mode == "first-fail":
        mode = "fail" if not prior_invocations else "pass"
    with open(invocation_log, "a", encoding="utf-8") as stream:
        stream.write(harness + "\\n")
record = {
    "status": "FAIL" if mode == "fail" else "PASS",
    "package": package,
    "expected_harnesses": [harness],
    "required_covers": {harness: covers},
    "loop_unwind_presets": {harness: loop_presets},
    "checks": [{"harness": harness, "status": "FAIL" if mode == "fail" else "PASS",
                "evaluation_errors": ["loop binding did not match source evidence"] if mode == "fail" else [],
                "source_unchanged": True}],
}
if mode == "wrong":
    record["expected_harnesses"] = ["other::proof"]
if mode != "missing":
    (out / "run.json").write_text(json.dumps(record))
print("fake runner stdout")
child_bytes = int(os.environ.get("SYNTHETIC_RESOURCE_CHILD_BYTES", "0"))
if child_bytes:
    code = "import sys; n=int(sys.argv[1]); b=bytearray(n); [b.__setitem__(i,1) for i in range(0,n,4096)]"
    subprocess.run([sys.executable, "-c", code, str(child_bytes)], check=True)
raise SystemExit(int(os.environ.get("SYNTHETIC_RESOURCE_RUNNER_EXIT", "0")))
'''


def make_workspace(root: Path, proof_count: int = 2) -> tuple[Path, Path, Path, Path]:
    repo = root / "repo"
    repo.mkdir()
    (repo / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
    tools = repo / "tools"
    tools.mkdir()
    (tools / "kani_validation.py").write_text(fake_runner_source(), encoding="utf-8")
    (tools / "rust_proof_suite.py").write_text("# placeholder checked-out suite source\n", encoding="utf-8")
    proofs = []
    for index in range(proof_count):
        proof = proof_entry()
        if index:
            proof["id"] = f"second-proof-{index}"
            proof["harness"] = f"types::proofs::example_{index}_bounded"
            proof["required_covers"] = []
        proofs.append(proof)
    registry = tools / "rust_proofs.json"
    registry.write_text(json.dumps(registry_with(*proofs), indent=2) + "\n", encoding="utf-8")
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
    return repo, registry, root / "evidence", root / "target"
