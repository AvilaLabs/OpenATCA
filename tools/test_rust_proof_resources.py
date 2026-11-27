"""Synthetic tests for per-proof peak and elapsed resource evidence."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

from rust_proof_suite import GNU_TIME_COMMAND, run_suite


def proof_entry() -> dict[str, object]:
    return {
        "id": "synthetic-resource-proof",
        "requirement": "Synthetic wrapper resource evidence.",
        "package": "oa-track",
        "harness": "synthetic::bounded_proof",
        "enabled": True,
        "experimental": False,
        "required_covers": ["one synthetic cover"],
        "stubbing": False,
        "stub_summaries": [],
        "cfgs": [],
        "harness_timeout_seconds": 60,
        "external_timeout_seconds": 120,
        "memory_mib": 4096,
        "tier": "push",
        "ci_seconds": 1,
        "ci_seconds_source": "ci",
    }


class ProofResourceEvidenceTests(unittest.TestCase):
    def make_workspace(self, root: Path) -> tuple[Path, Path, Path, Path]:
        repo = root / "repo"
        tools = repo / "tools"
        tools.mkdir(parents=True)
        (repo / "Cargo.toml").write_text('[workspace]\nmembers = []\n', encoding="utf-8")
        (tools / "rust_proof_suite.py").write_text("# synthetic suite source\n", encoding="utf-8")
        fake_runner = tools / "kani_validation.py"
        fake_runner.write_text(
            "import json, pathlib, sys\n"
            "import os, subprocess\n"
            "args=sys.argv[1:]\n"
            "def value(flag): return args[args.index(flag)+1]\n"
            "out=pathlib.Path(value('--out')); out.mkdir(parents=True, exist_ok=True)\n"
            "harness=value('--harness'); package=value('--package')\n"
            "covers=[args[i+1].split('=',1)[1] for i,x in enumerate(args[:-1]) if x=='--require-cover']\n"
            "record={'status':'PASS','package':package,'expected_harnesses':[harness],"
            "'required_covers':{harness:covers},'loop_unwind_presets':{harness:[]}}\n"
            "(out/'run.json').write_text(json.dumps(record))\n",
            encoding="utf-8",
        )
        source = fake_runner.read_text(encoding="utf-8")
        source += (
            "child_bytes=int(os.environ.get('SYNTHETIC_RESOURCE_CHILD_BYTES','0'))\n"
            "if child_bytes:\n"
            " code=\"import sys; n=int(sys.argv[1]); b=bytearray(n); "
            "[b.__setitem__(i,1) for i in range(0,n,4096)]\"\n"
            " subprocess.run([sys.executable,'-c',code,str(child_bytes)],check=True)\n"
            "raise SystemExit(int(os.environ.get('SYNTHETIC_RESOURCE_RUNNER_EXIT','0')))\n"
        )
        fake_runner.write_text(source, encoding="utf-8")
        registry = tools / "rust_proofs.json"
        registry.write_text(
            json.dumps({"registry_version": 1, "expected_proof_count": 1, "proofs": [proof_entry()]}),
            encoding="utf-8",
        )
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
        out, target = root / "evidence", root / "target"
        return repo, registry, out, target

    def make_time_wrapper(self, root: Path, mode: str) -> Path:
        wrapper = root / "synthetic-time"
        wrapper.write_text(
            "#!/usr/bin/env python3\n"
            "import os, subprocess, sys\n"
            "args=sys.argv[1:]\n"
            "if args == ['--version']:\n"
            " print('time (GNU Time) synthetic test wrapper'); raise SystemExit(0)\n"
            "out=args[args.index('-o')+1]; command=args[args.index('--')+1:]\n"
            f"mode={mode!r}\n"
            "if mode == 'valid': open(out,'w').write('OPENATCA_PROOF_RESOURCE_V1 123 0.01\\n')\n"
            "elif mode == 'malformed': open(out,'w').write('not a resource record\\n')\n"
            "raise SystemExit(subprocess.run(command).returncode)\n",
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
        return wrapper

    def test_full_suite_binds_gnu_time_peak_and_elapsed_file_to_proof_outcome(self) -> None:
        if shutil.which(GNU_TIME_COMMAND) is None:
            self.skipTest("GNU time is unavailable on this host")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo, registry, out, target = self.make_workspace(root)
            result = run_suite(repo, out, target, registry)
            suite = json.loads((out / "suite.json").read_text(encoding="utf-8"))
            self.assertEqual(result, 0)
            self.assertEqual(suite["status"], "PASS")
            proof = suite["checks"][0]
            resource_path = out / proof["resource_file"]
            resource_bytes = resource_path.read_bytes()
            self.assertEqual(proof["resource_file_sha256"], hashlib.sha256(resource_bytes).hexdigest())
            resource = json.loads(resource_bytes)
            self.assertEqual(resource["status"], "available")
            self.assertGreater(resource["measurement"]["maximum_rss_kib"], 0)
            self.assertGreaterEqual(resource["measurement"]["elapsed_seconds"], 0)
            self.assertIn("largest individual waited child", resource["measurement"]["measurement_scope"])
            self.assertTrue(resource["tool"]["binary_sha256"])

    def test_resource_peak_includes_waited_descendant_allocation(self) -> None:
        """Kills a direct-runner-only peak interpretation by allocating in a waited child."""
        if shutil.which(GNU_TIME_COMMAND) is None:
            self.skipTest("GNU time is unavailable on this host")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo, registry, out, target = self.make_workspace(root)
            child_allocation_bytes = 64 * 1024 * 1024
            with mock.patch.dict(os.environ, {"SYNTHETIC_RESOURCE_CHILD_BYTES": str(child_allocation_bytes)}):
                result = run_suite(repo, out, target, registry)
            suite = json.loads((out / "suite.json").read_text(encoding="utf-8"))
            measurement = suite["checks"][0]["resource_measurement"]["measurement"]
            self.assertEqual(result, 0)
            self.assertGreater(measurement["maximum_rss_kib"], child_allocation_bytes * 3 // (4 * 1024))

    def test_nonzero_runner_exit_keeps_resource_record_but_still_fails_proof(self) -> None:
        """Kills omission of GNU time's quiet mode, which prefixes nonzero child status text."""
        if shutil.which(GNU_TIME_COMMAND) is None:
            self.skipTest("GNU time is unavailable on this host")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo, registry, out, target = self.make_workspace(root)
            with mock.patch.dict(os.environ, {"SYNTHETIC_RESOURCE_RUNNER_EXIT": "7"}):
                result = run_suite(repo, out, target, registry)
            suite = json.loads((out / "suite.json").read_text(encoding="utf-8"))
            proof = suite["checks"][0]
            self.assertEqual(result, 1)
            self.assertEqual(proof["status"], "FAIL")
            self.assertEqual(proof["exit_code"], 7)
            self.assertEqual(proof["resource_measurement"]["status"], "available")

    def test_missing_or_malformed_resource_output_cannot_earn_pass(self) -> None:
        for mode in ("missing", "malformed"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                repo, registry, out, target = self.make_workspace(root)
                wrapper = self.make_time_wrapper(root, mode)
                with mock.patch("rust_proof_suite.GNU_TIME_COMMAND", str(wrapper)):
                    result = run_suite(repo, out, target, registry)
                suite = json.loads((out / "suite.json").read_text(encoding="utf-8"))
                proof = suite["checks"][0]
                self.assertEqual(result, 1)
                self.assertEqual(suite["status"], "FAIL")
                self.assertEqual(proof["status"], "FAIL")
                self.assertEqual(proof["resource_measurement"]["status"], "unavailable")
                self.assertTrue(any(
                    diagnostic.startswith("per-proof peak/time measurement unavailable")
                    for diagnostic in proof["diagnostics"]
                ))


if __name__ == "__main__":
    unittest.main()
