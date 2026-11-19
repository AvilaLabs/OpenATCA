"""The commit-spec and argument checks of replay_compare.sh (no build is started)."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent / "replay_compare.sh"


def run(*args, cwd=None):
    return subprocess.run(
        ["bash", str(SCRIPT), *args],
        capture_output=True,
        text=True,
        cwd=cwd,
        timeout=60,
    )


class ReplayCompareArguments(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.clip = self.dir / "clip.oaf"
        self.clip.write_bytes(b"x")
        self.out = self.dir / "out"

    def tearDown(self):
        self.tmp.cleanup()

    # REQ: OA-RDF-006
    def test_a_spec_that_is_not_a_hash_or_ref_is_refused_before_anything_is_built(self):
        for bad in [
            "HEAD~1",
            "HEAD^",
            "main..HEAD",
            "v1-dirty",
            "main@{1}",
            "a b",
            "HEAD:file",
            ".",
            "",
        ]:
            with self.subTest(spec=bad):
                r = run(bad, "HEAD", str(self.clip), str(self.out))
                self.assertEqual(r.returncode, 2, r.stderr)
                self.assertIn("not a commit hash or ref", r.stderr)
                self.assertFalse((self.out / "work").exists())
        for opt in ["--help", "-x"]:
            r = run(opt, "HEAD", str(self.clip), str(self.out))
            self.assertEqual(r.returncode, 2)
            self.assertIn("usage", r.stderr)
        r = run("HEAD", "no-such-ref-for-the-test", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such commit", r.stderr)

    # REQ: OA-RDF-006
    def test_missing_clip_wrong_argument_count_and_existing_report_are_refused(self):
        r = run("HEAD", "HEAD", str(self.dir / "nope.oaf"), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such clip", r.stderr)
        r = run("HEAD", "HEAD")
        self.assertEqual(r.returncode, 2)
        self.assertIn("usage", r.stderr)
        r = run("HEAD", "HEAD", str(self.clip), str(self.out), "--expect", "missing.txt")
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such expect file", r.stderr)
        self.out.mkdir()
        (self.out / "report.txt").write_text("old")
        r = run("HEAD", "HEAD", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("exists", r.stderr)
        self.assertEqual((self.out / "report.txt").read_text(), "old")


if __name__ == "__main__":
    unittest.main()
