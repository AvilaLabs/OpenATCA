"""Tests for the synthetic proof driver; E07 uses the production CLI in a tiny Git workspace."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools import prove_quality_rejections as proof


class QualityRejectionProofTests(unittest.TestCase):
    def test_e07_proof_driver_exits_on_verified_cli_refusals(self) -> None:
        result = subprocess.run(
            [sys.executable, str(Path(proof.__file__)), "--class", "E07"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("class=E07", result.stdout)
        self.assertIn("long-function,wide-arguments,duplicate-block-refused", result.stdout)
        self.assertIn("fixture_sha256=", result.stdout)

    def test_e05_driver_proves_external_refusal_with_actual_trace_binary(self) -> None:
        binary = os.environ.get("OPENATCA_OA_TRACE_BIN")
        if not binary or not Path(binary).is_file() or not os.access(binary, os.X_OK):
            self.skipTest("qualified oa-trace executable is not configured")
        digest, refused = proof._prove_e05(Path(__file__).resolve().parents[1])
        self.assertEqual(len(digest), 64)
        self.assertEqual(refused, "external-clause-refused")

    def test_e05_requires_explicit_trace_executable_configuration(self) -> None:
        env = os.environ.copy()
        env.pop("OPENATCA_OA_TRACE_BIN", None)
        result = subprocess.run(
            [sys.executable, str(Path(proof.__file__)), "--class", "E05"],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("OPENATCA_OA_TRACE_BIN", result.stderr)

    def test_unknown_class_is_a_setup_error(self) -> None:
        result = subprocess.run(
            [sys.executable, str(Path(proof.__file__)), "--class", "E06"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)

    def test_e09_actual_clis_refuse_untrusted_roots_without_inherited_config(self) -> None:
        """Kill ambient OPENATCA_DATA inheritance masking a missing-root refusal."""
        binary = os.environ.get("OPENATCA_OA_TRACE_BIN")
        if not binary or not Path(binary).is_file() or not os.access(binary, os.X_OK):
            self.skipTest("qualified oa-trace executable is not configured")
        with tempfile.TemporaryDirectory(prefix="oa-e09-inherited-root-") as temporary:
            inherited = Path(temporary)
            (inherited / ".openatca-data-root").write_bytes(b"openatca-data-root-v1\n")
            with patch.dict(os.environ, {"OPENATCA_DATA": str(inherited)}):
                self.assertNotIn("OPENATCA_DATA", proof._e09_env(None))
                digest, refused = proof._prove_e09(Path(__file__).resolve().parents[1])
        self.assertEqual(len(digest), 64)
        for label in ("markerless", "wrong-marker", "missing-root", "slash-root", "empty-configuration"):
            self.assertIn(f"python-{label}", refused)
            self.assertIn(f"trace-{label}", refused)

    def test_e09_requires_explicit_trace_executable_configuration(self) -> None:
        env = os.environ.copy()
        env.pop("OPENATCA_OA_TRACE_BIN", None)
        result = subprocess.run(
            [sys.executable, str(Path(proof.__file__)), "--class", "E09"],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("OPENATCA_OA_TRACE_BIN", result.stderr)


if __name__ == "__main__":
    unittest.main()
