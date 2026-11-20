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
    def test_the_list_parses_to_the_sealed_entries(self):
        root, entries = sealed_guard.parse_list(sealed_guard.LIST_PATH.read_text())
        self.assertEqual([e[0] for e in entries], [
            "scds/rec-20261119", "scds/stdds-20261119-holdout", "scds/stdds-smoke-20261119*",
            "raw/adsblol/2026.11.20", "raw/adsblol/2026.11.21", "raw/adsblol/2026.11.22"])
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

    # REQ: OA-SEAL-007
    def test_a_file_swapped_in_after_the_check_is_caught_on_the_opened_descriptor(self):
        os.symlink(self.root / "scds/rec-20261119/sfdps/a.bin", self.base / "late")
        g = sealed_guard.Guard(root=self.root)
        g.check = lambda p: None  # as if the link was swapped in after the check
        with self.assertRaises(sealed_guard.SealedError) as c:
            g.open(self.base / "late", "rb")
        self.assertIn("OA-SEAL-001", str(c.exception))
        # Write modes are not offered; a bad path is a SealedError, not a traceback.
        with self.assertRaises(ValueError):
            self.g.open(self.base / "x", "w")
        for bad in ("a\0b", ""):
            if bad:
                self.refused(bad)

    # REQ: OA-SEAL-005
    def test_each_argument_of_each_python_tool_is_refused_on_its_own(self):
        here = Path(__file__).resolve().parent
        t = self.base / "tree"
        sealed = t / "scds" / "rec-20261119" / "x"
        sealed.mkdir(parents=True)
        (sealed / "f.csv").write_bytes(b"x")
        open_dir = t / "open"
        open_dir.mkdir()
        os.symlink(sealed / "f.csv", open_dir / "file-link")
        os.symlink(sealed, open_dir / "dir-link")
        os.symlink(t / "scds", open_dir / "parent-link")
        benign = str(self.base / "benign")
        tools = {
            "track_set_score.py": ["--truth", "--outputs", "--manifest", "--out"],
            "comparison_reference.py": ["--truth", "--measurements", "--manifest", "--protocol", "--out", "--runner-source"],
            "comparison_evaluator.py": ["--truth", "--measurements", "--candidate", "--baseline", "--metadata", "--out"],
            "validate_candidate.py": ["--repo", "--target-dir", "--out", "--reference-env", "--clip"],
            "run_comparison.py": ["--repo", "--out", "--target-dir", "--runner"],
        }
        fixed = {"validate_candidate.py": ["--revision", "x"]}
        required = {
            "validate_candidate.py": ["--repo", "--target-dir", "--out"],
            "run_comparison.py": ["--out"],
        }
        for tool, flags in tools.items():
            base_args = {f: benign for f in flags}
            if tool in required:
                base_args = {f: benign for f in flags if f in required[tool]}
            for flag in flags:
                for link in (open_dir / "file-link", open_dir / "dir-link", open_dir / "parent-link" / "rec-20261119" / "x" / "f.csv"):
                    args = dict(base_args)
                    args[flag] = str(link)
                    argv = [a for kv in args.items() for a in kv] + fixed.get(tool, [])
                    r = subprocess.run([sys.executable, str(here / tool), *argv], capture_output=True, text=True)
                    self.assertEqual(r.returncode, 2, (tool, flag, str(link), r.stderr))
                    self.assertIn("OA-SEAL-001", r.stderr, (tool, flag))

    # REQ: OA-SEAL-005
    def test_validate_candidate_guards_every_path_it_takes_from_the_reference_env(self):
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import validate_candidate as vc
        t = self.base / "tree2"
        sealed = t / "scds" / "stdds-20261119-holdout"
        sealed.mkdir(parents=True)
        (sealed / "clip.oaf").write_bytes(b"x")
        link = self.base / "clip-link.oaf"
        os.symlink(sealed / "clip.oaf", link)
        dlink = self.base / "dir-link"
        os.symlink(sealed, dlink)
        for key, value in (("OPENATCA_ADSB_CLIP", link), ("OPENATCA_RAW", dlink), ("OPENATCA_DATA", dlink)):
            with self.assertRaises(sealed_guard.SealedError, msg=key):
                vc.resolve_reference_paths({key: str(value)}, self.base)
        with self.assertRaises(sealed_guard.SealedError):
            vc.directory_manifest(dlink, {})
        with self.assertRaises(sealed_guard.SealedError):
            vc.sha(link)

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

    # REQ: OA-SEAL-008
    def test_odd_list_files_are_refused_by_the_python_parser(self):
        for bad in ("root /d\nseal a\tb\n", "root /d\r\nseal a\n", "root  /d\nseal a\n", "root /d\nseal a \n",
                    "root /d\n seal a\n", "root /d\nseal \x00a\n", "root /d\nseal a\x0bb\n", "root /d\nseal a\x1cb\n",
                    "root /d\nseal \u00e9\n", "root /d\nseal a\u2028b\n", "root /d\n \nseal a\n", "root /d\nseal a//b\n"):
            with self.assertRaises(ValueError, msg=repr(bad)):
                sealed_guard.parse_list(bad)
        sealed_guard.parse_list("# any printable ~!@ in a comment\nroot /d\nseal a/b*")


if __name__ == "__main__":
    unittest.main()
