from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "prepare-transfer-bundle.py"
SPEC = importlib.util.spec_from_file_location("prepare_transfer_bundle", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class TransferBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.feed = self.root / "oa-feed.synthetic"
        self.jar = self.root / "bridge.synthetic.jar"
        self.feed.write_bytes(b"synthetic approved oa-feed executable\n")
        self.jar.write_bytes(b"synthetic approved bridge jar\n")
        self.feed.chmod(0o700)
        self.jar.chmod(0o600)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def args(self, output: Path, *, feed_pin: str | None = None) -> list[str]:
        return [
            "--oa-feed", str(self.feed),
            "--oa-feed-sha256", feed_pin or hashlib.sha256(self.feed.read_bytes()).hexdigest(),
            "--bridge-jar", str(self.jar),
            "--bridge-jar-sha256", hashlib.sha256(self.jar.read_bytes()).hexdigest(),
            "--output", str(output),
        ]

    def test_prepares_pinned_bundle_without_subscription_config(self) -> None:
        output = self.root / "bundle"
        self.assertEqual(MODULE.main(self.args(output)), 0)
        manifest = json.loads((output / "SHA256SUMS.json").read_text())
        rows = {item["path"]: item for item in manifest["files"]}
        self.assertEqual(rows["bin/oa-feed"]["sha256"],
                         hashlib.sha256(self.feed.read_bytes()).hexdigest())
        self.assertEqual(rows["lib/scds-bridge.jar"]["sha256"],
                         hashlib.sha256(self.jar.read_bytes()).hexdigest())
        self.assertTrue((output / "config/storage-budget.env.example").is_file())
        self.assertFalse((output / "config/SFDPS.env").exists())
        self.assertEqual(manifest["capture_acquisition_utc"],
                         ["2026-11-30T05:00:00Z", "2026-12-01T06:00:00Z"])
        self.assertEqual(manifest["warmup_excluded_from_scoring_utc"],
                         ["2026-11-30T05:00:00Z", "2026-11-30T06:00:00Z"])
        self.assertEqual(manifest["scored_interval_utc"],
                         ["2026-11-30T06:00:00Z", "2026-12-01T00:00:00Z"])
        self.assertEqual(manifest["capture_acquisition_duration_hours"], 25)
        self.assertEqual(manifest["scored_interval_duration_hours"], 18)
        self.assertEqual(manifest["storage_planning_horizon_hours"], 720)
        self.assertEqual(rows["config/storage-budget.env.example"]["bytes"],
                         (output / "config/storage-budget.env.example").stat().st_size)
        self.assertTrue(manifest["prepared_only"])
        self.assertFalse(manifest["transfer_or_service_action_performed"])

    def test_wrong_pin_refuses_without_publishing_partial_bundle(self) -> None:
        output = self.root / "wrong-pin-bundle"
        args = self.args(output, feed_pin="0" * 64)
        self.assertEqual(MODULE.main(args), 2)
        self.assertFalse(output.exists())
        self.assertEqual(list(self.root.glob(".wrong-pin-bundle.stage-*")), [])

    def test_existing_output_is_never_overwritten(self) -> None:
        output = self.root / "existing"
        output.mkdir()
        sentinel = output / "sentinel"
        sentinel.write_text("preserve")
        self.assertEqual(MODULE.main(self.args(output)), 2)
        self.assertEqual(sentinel.read_text(), "preserve")

    def test_symlink_inputs_are_refused(self) -> None:
        link = self.root / "feed-link"
        link.symlink_to(self.feed)
        args = self.args(self.root / "symlink-bundle")
        args[1] = str(link)
        self.assertEqual(MODULE.main(args), 2)
        self.assertFalse((self.root / "symlink-bundle").exists())

    @unittest.skipUnless(
        hasattr(os, "mkfifo") and hasattr(os, "O_NONBLOCK"),
        "nonblocking FIFO open is unavailable",
    )
    def test_fifo_input_is_refused_without_waiting_for_a_writer(self) -> None:
        fifo = self.root / "feed.fifo"
        os.mkfifo(fifo)
        args = self.args(self.root / "fifo-bundle")
        args[1] = str(fifo)
        self.assertEqual(MODULE.main(args), 2)
        self.assertFalse((self.root / "fifo-bundle").exists())

    def test_parent_fsync_failure_reports_published_bundle_and_preserves_it(self) -> None:
        output = self.root / "published-bundle"
        original = MODULE.fsync_directory

        def fail_parent(path: Path) -> None:
            if path == self.root:
                raise OSError("synthetic parent fsync failure")
            original(path)

        with mock.patch.object(MODULE, "fsync_directory", side_effect=fail_parent):
            self.assertEqual(MODULE.main(self.args(output)), 3)
        self.assertTrue((output / "SHA256SUMS.json").is_file())
        self.assertTrue((output / "bin/oa-feed").is_file())
        self.assertEqual(list(self.root.glob(".published-bundle.stage-*")), [])

    def test_oversized_explicit_input_is_refused(self) -> None:
        self.feed.write_bytes(b"synthetic")
        original_bound = MODULE.MAX_FEED_BYTES
        MODULE.MAX_FEED_BYTES = 4
        args = self.args(self.root / "oversized-bundle")
        try:
            self.assertEqual(MODULE.main(args), 2)
            self.assertFalse((self.root / "oversized-bundle").exists())
        finally:
            MODULE.MAX_FEED_BYTES = original_bound


if __name__ == "__main__":
    unittest.main()
