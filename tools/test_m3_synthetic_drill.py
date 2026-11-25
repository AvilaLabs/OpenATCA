"""Source-only tests for the M3 evidence writer's no-overwrite contract."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "run_m3_synthetic_drill", Path(__file__).with_name("run_m3_synthetic_drill.py"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class EvidenceWriterTests(unittest.TestCase):
    def test_new_report_and_parent_are_private(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "fresh" / "report.json"
            runner.write_report(output, {"schema": "test"})
            self.assertEqual(output.read_text(encoding="utf-8"), '{"schema":"test"}\n')
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            self.assertEqual(output.parent.stat().st_mode & 0o777, 0o700)

    def test_existing_report_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "report.json"
            output.write_bytes(b"prior evidence")
            with self.assertRaises(ValueError):
                runner.write_report(output, {"schema": "replacement"})
            self.assertEqual(output.read_bytes(), b"prior evidence")

    def test_concurrent_creator_wins_without_evidence_replacement(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "report.json"
            original_open = os.open
            original_link = os.link

            def racing_link(source, destination, *, follow_symlinks=True):
                if Path(destination) == output:
                    race_fd = original_open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    os.write(race_fd, b"concurrent evidence")
                    os.close(race_fd)
                    return original_link(source, destination, follow_symlinks=follow_symlinks)
                return original_link(source, destination, follow_symlinks=follow_symlinks)

            with patch.object(runner.os, "link", side_effect=racing_link):
                with self.assertRaises(FileExistsError):
                    runner.write_report(output, {"schema": "losing writer"})
            self.assertEqual(output.read_bytes(), b"concurrent evidence")
            partials = list(Path(temp).glob(".report.json.partial-*"))
            self.assertEqual(len(partials), 1)

    def test_failed_serialization_preserves_partial_and_never_publishes_final(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "report.json"

            def fail_after_partial(fd, _encoded):
                os.write(fd, b"partial evidence")
                os.close(fd)
                raise OSError("injected write failure")

            with patch.object(runner, "write_temp_bytes", side_effect=fail_after_partial):
                with self.assertRaisesRegex(OSError, "injected write failure"):
                    runner.write_report(output, {"schema": "failed write"})
            self.assertFalse(output.exists())
            partials = list(Path(temp).glob(".report.json.partial-*"))
            self.assertEqual(len(partials), 1)
            self.assertEqual(partials[0].read_bytes(), b"partial evidence")


if __name__ == "__main__":
    unittest.main()
