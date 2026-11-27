"""Synthetic tests for per-proof peak and elapsed resource evidence."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import ExitStack
from unittest import mock

from rust_proof_suite import GNU_TIME_COMMAND, run_suite
from proof_suite_test_support import make_workspace, proof_entry, registry_with


FIXED_RSS_KIB = 123
FIXED_ELAPSED_SECONDS = 0.01


class ProofResourceEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self._temporary.cleanup)
        self.root = Path(self._temporary.name)
        self.repo, self.registry, self.output, self.target = self.make_workspace(self.root)

    def make_workspace(self, root: Path) -> tuple[Path, Path, Path, Path]:
        proof = proof_entry({
            "id": "synthetic-resource-proof",
            "requirement": "Synthetic wrapper resource evidence.",
            "harness": "synthetic::bounded_proof",
            "required_covers": ["one synthetic cover"],
            "harness_timeout_seconds": 60,
            "external_timeout_seconds": 120,
            "ci_seconds": 1,
            "ci_seconds_source": "ci",
        })
        repo, registry, output, target = make_workspace(root, proof_count=1)
        registry.write_text(json.dumps(registry_with(proof), indent=2) + "\n", encoding="utf-8")
        return repo, registry, output, target

    def execute_suite(self, env=None, time_command=None, workspace=None):
        repo, registry, output, target = workspace or (
            self.repo, self.registry, self.output, self.target,
        )
        with ExitStack() as contexts:
            contexts.enter_context(mock.patch.dict(os.environ, env or {}))
            if time_command is not None:
                contexts.enter_context(mock.patch("rust_proof_suite.GNU_TIME_COMMAND", time_command))
            result = run_suite(repo, output, target, registry)
        suite = json.loads((output / "suite.json").read_text(encoding="utf-8"))
        return result, suite, output

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
        result, suite, output = self.execute_suite()
        self.assertEqual(result, 0)
        self.assertEqual(suite["status"], "PASS")
        proof = suite["checks"][0]
        resource_path = output / proof["resource_file"]
        resource_bytes = resource_path.read_bytes()
        self.assertEqual(proof["resource_file_sha256"], hashlib.sha256(resource_bytes).hexdigest())
        resource = json.loads(resource_bytes)
        self.assertEqual(resource["status"], "available")
        self.assertGreater(resource["measurement"]["maximum_rss_kib"], 0)
        self.assertGreaterEqual(resource["measurement"]["elapsed_seconds"], 0)
        self.assertIn("largest individual waited child", resource["measurement"]["measurement_scope"])
        self.assertTrue(resource["tool"]["binary_sha256"])

    def test_fixed_raw_receipt_decodes_exact_rss_and_elapsed(self) -> None:
        root = self.root / "fixed-receipt"
        root.mkdir()
        workspace = self.make_workspace(root)
        wrapper = self.make_time_wrapper(root, "valid")
        result, suite, _ = self.execute_suite(
            time_command=str(wrapper), workspace=workspace,
        )
        measurement = suite["checks"][0]["resource_measurement"]["measurement"]
        self.assertEqual(result, 0)
        self.assertEqual(measurement["maximum_rss_kib"], FIXED_RSS_KIB)
        self.assertEqual(measurement["elapsed_seconds"], FIXED_ELAPSED_SECONDS)

    def test_resource_peak_includes_waited_descendant_allocation(self) -> None:
        """Kills a direct-runner-only peak interpretation by allocating in a waited child."""
        if shutil.which(GNU_TIME_COMMAND) is None:
            self.skipTest("GNU time is unavailable on this host")
        child_allocation_bytes = 64 * 1024 * 1024
        result, suite, _ = self.execute_suite(
            env={"SYNTHETIC_RESOURCE_CHILD_BYTES": str(child_allocation_bytes)},
        )
        measurement = suite["checks"][0]["resource_measurement"]["measurement"]
        self.assertEqual(result, 0)
        self.assertGreater(measurement["maximum_rss_kib"], child_allocation_bytes * 3 // (4 * 1024))

    def test_nonzero_runner_exit_keeps_resource_record_but_still_fails_proof(self) -> None:
        """Kills omission of GNU time's quiet mode, which prefixes nonzero child status text."""
        if shutil.which(GNU_TIME_COMMAND) is None:
            self.skipTest("GNU time is unavailable on this host")
        result, suite, _ = self.execute_suite(env={"SYNTHETIC_RESOURCE_RUNNER_EXIT": "7"})
        proof = suite["checks"][0]
        self.assertEqual(result, 1)
        self.assertEqual(proof["status"], "FAIL")
        self.assertEqual(proof["exit_code"], 7)
        self.assertEqual(proof["resource_measurement"]["status"], "available")

    def test_missing_or_malformed_resource_output_cannot_earn_pass(self) -> None:
        for mode in ("missing", "malformed"):
            with self.subTest(mode=mode):
                root = self.root / mode
                root.mkdir()
                repo, registry, output, target = self.make_workspace(root)
                wrapper = self.make_time_wrapper(root, mode)
                result, suite, _ = self.execute_suite(
                    time_command=str(wrapper), workspace=(repo, registry, output, target),
                )
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
