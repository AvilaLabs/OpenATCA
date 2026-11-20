"""The Python sealed-data guard, on fake data roots in temp directories (never the real ones)."""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard  # noqa: E402


def fake_root(base: Path) -> Path:
    root = base / "data"
    for d in ("scds/rec-20261119/sfdps", "scds/stdds-20261119-holdout",
              "scds/stdds-smoke-20261119T192936Z", "scds/day-20261118/rec"):
        (root / d).mkdir(parents=True)
    (root / "scds/rec-20261119/sfdps/a.bin").write_bytes(b"x")
    (root / "scds/day-20261118/rec/b.bin").write_bytes(b"y")
    return root


class SealedGuardTests(unittest.TestCase):
    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.base = Path(self._t.name).resolve()
        self.root = fake_root(self.base)
        self.g = sealed_guard.Guard(root=self.root)

    def tearDown(self):
        self._t.cleanup()

    def refused(self, p):
        with self.assertRaises(sealed_guard.SealedError) as c:
            self.g.check(p)
        self.assertIn("OA-SEAL-001", str(c.exception))

    # REQ: OA-SEAL-001
    def test_the_list_parses_to_the_three_entries(self):
        root, entries = sealed_guard.parse_list(sealed_guard.LIST_PATH.read_text())
        self.assertEqual([e[0] for e in entries], [
            "scds/rec-20261119", "scds/stdds-20261119-holdout", "scds/stdds-smoke-20261119*"])
        for bad in ("", "root /d\n", "seal a/b\n", "root /d\nseal a/../b\n", "root d\nseal a\n",
                    "root /d\nseal *\n", "root /d\nfoo bar\n"):
            with self.assertRaises(ValueError):
                sealed_guard.parse_list(bad)
        broken = sealed_guard.Guard(text="garbage")
        with self.assertRaises(sealed_guard.SealedError):
            broken.check("/tmp/anything")

    # REQ: OA-SEAL-002
    def test_names_as_given_and_near_misses(self):
        for p in ("x/scds/rec-20261119", "/data/scds/rec-20261119/sub/f", "/data/scds/stdds-20261119-holdout/a",
                  "/data/scds/stdds-smoke-20261119T192936Z/b", "/data/scds/stdds-smoke-20261119"):
            self.refused(p)
        for p in ("/data/scds/day-20261114/rec", "/data/scds/rec-20261119x", "/data/rec-20261119",
                  "/data/scds/x/rec-20261119", "/data/scds/stdds-smoke-20261118T1"):
            self.g.check(p)

    # REQ: OA-SEAL-002
    def test_paths_are_resolved_before_the_check(self):
        work = self.base / "work"
        work.mkdir()
        sealed = self.root / "scds/rec-20261119"
        os.symlink(sealed, work / "dir")
        os.symlink(sealed / "sfdps/a.bin", work / "file")
        os.symlink(work / "dir", work / "chain")
        for p in (work / "dir", work / "dir/sfdps/a.bin", work / "file", work / "chain/sfdps",
                  self.root / "scds/day-20261118/../rec-20261119/sfdps",
                  work / "dir/new-output", work / "dir/not/yet/here",
                  self.root / "scds/day-20261118/nothing/../../rec-20261119"):
            self.refused(p)
        rel = os.path.relpath(sealed, os.getcwd())
        self.refused(rel)
        os.symlink(self.root / "scds/day-20261118", work / "dev")
        self.g.check(work / "dev/rec/b.bin")
        self.g.check(work / "new-file")

    # REQ: OA-SEAL-002
    def test_unresolvable_links_and_holding_directories(self):
        work = self.base / "work"
        work.mkdir()
        os.symlink(work / "b", work / "a")
        os.symlink(work / "a", work / "b")
        self.refused(work / "a")
        os.symlink(self.root / "scds/stdds-20261119-holdout/later", work / "dangling")
        self.refused(work / "dangling")
        os.symlink(work / "nowhere", work / "plain")
        self.g.check(work / "plain")
        self.refused(self.root)
        self.refused(self.root / "scds")
        self.g.check(self.root / "scds/day-20261118")
        self.g.check(self.base)

    # REQ: OA-SEAL-003
    def test_the_opener_refuses_before_it_opens(self):
        dev = self.root / "scds/day-20261118/rec"
        os.symlink(self.root / "scds/rec-20261119/sfdps/a.bin", dev / "sneaky")
        read, refused = 0, 0
        for p in sorted(dev.iterdir()):
            try:
                with self.g.open(p, "rb"):
                    read += 1
            except sealed_guard.SealedError:
                refused += 1
        self.assertEqual((read, refused), (1, 1))

    # REQ: OA-SEAL-004
    def test_nothing_switches_the_guard_off(self):
        src = (Path(sealed_guard.__file__)).read_text()
        for word in ("os.environ", "getenv", "argparse"):
            self.assertNotIn(word, src)
        r = subprocess.run([sys.executable, "-I", sealed_guard.__file__, "--off", "x"],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        sealed_dir = self.root / "scds/rec-20261119/sfdps/a.bin"
        self.refused(sealed_dir)

    # REQ: OA-SEAL-005
    def test_the_command_line_checker_exits_2_naming_the_rule(self):
        # Fake paths that only name a sealed folder: nothing real is touched.
        r = subprocess.run([sys.executable, "-I", sealed_guard.__file__,
                            "/nonexistent-fake/scds/stdds-20261119-holdout/x"],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr)
        ok = subprocess.run([sys.executable, "-I", sealed_guard.__file__, str(self.base / "fine")],
                            capture_output=True, text=True)
        self.assertEqual(ok.returncode, 0)

    # REQ: OA-SEAL-005
    def test_the_shell_scripts_refuse_a_sealed_path_before_anything_is_created(self):
        here = Path(__file__).resolve().parent
        out = self.base / "out"
        # Fake paths that only name a sealed folder: nothing real is touched.
        r = subprocess.run(["bash", str(here / "replay_compare.sh"), "HEAD", "HEAD",
                            "/nonexistent-fake/scds/rec-20261119/clip.oaf", str(out)],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr)
        self.assertFalse(out.exists())
        # An output directory under a link to a sealed folder is refused as well.
        os.symlink(self.root / "scds/rec-20261119", self.base / "lnk")
        r = subprocess.run(["bash", str(here / "replay_compare.sh"), "HEAD", "HEAD",
                            str(self.base / "clip.oaf"), str(self.base / "lnk/out")],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr)
        r = subprocess.run(["bash", str(here / "p002_build.sh"), "/nonexistent-fake/scds/stdds-20261119-holdout/x"],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr)

    # REQ: OA-SEAL-005
    def test_the_python_tools_refuse_a_sealed_path_with_exit_2_and_write_nothing(self):
        here = Path(__file__).resolve().parent
        fake = "/nonexistent-fake/scds/stdds-smoke-20261119T192936Z/x.csv"
        out = self.base / "o.json"
        for tool, argv in (
            ("track_set_score.py", ["--truth", fake, "--outputs", fake, "--manifest", fake, "--out", str(out)]),
            ("comparison_reference.py", ["--truth", fake, "--measurements", fake, "--manifest", fake,
                                         "--protocol", fake, "--out", str(out)]),
            ("comparison_evaluator.py", ["--truth", fake, "--measurements", fake, "--candidate", fake,
                                         "--baseline", fake, "--metadata", fake, "--out", str(out)]),
            ("validate_candidate.py", ["--repo", str(here.parent), "--revision", "x", "--target-dir",
                                       str(self.base / "t"), "--out", str(self.base / "gate"), "--clip", fake]),
            ("run_comparison.py", ["--out", str(self.base / "run"), "--runner", fake]),
        ):
            r = subprocess.run([sys.executable, str(here / tool), *argv], capture_output=True, text=True)
            self.assertEqual(r.returncode, 2, (tool, r.stderr))
            self.assertIn("OA-SEAL-001", r.stderr, tool)
        self.assertEqual(list(self.base.glob("o.json")) + list(self.base.glob("gate")) + list(self.base.glob("run")), [])


if __name__ == "__main__":
    unittest.main()
