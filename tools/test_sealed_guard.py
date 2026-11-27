"""The Python sealed-data guard, on fake data roots in temp directories (never the real ones)."""
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard  # noqa: E402

ROOT_SENTINEL_FILE = ".openatca-data-root"
ROOT_SENTINEL_FIXTURE = b"openatca-data-root-v1\n"


def fake_root(base: Path) -> Path:
    root = base / "data"
    for d in ("scds/rec-20261119/day2/sfdps", "scds/rec-20261119/day1", "scds/rec-20261119/bin",
              "scds/stdds-20261119-holdout",
              "scds/stdds-smoke-20261119T192936Z", "scds/day-20261118/rec"):
        (root / d).mkdir(parents=True)
    (root / "scds/rec-20261119/day2/sfdps/a.bin").write_bytes(b"x")
    (root / "scds/day-20261118/rec/b.bin").write_bytes(b"y")
    (root / ROOT_SENTINEL_FILE).write_bytes(ROOT_SENTINEL_FIXTURE)
    return root


class SealedGuardTests(unittest.TestCase):
    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.base = Path(self._t.name).resolve()
        self.root = fake_root(self.base)
        self.g = sealed_guard.Guard(root=self.root)

    def test_configured_data_root_defaults_to_repository_relative_and_refuses_real_paths(self):
        expected = sealed_guard._resolve(os.fspath(sealed_guard.DEFAULT_DATA_ROOT))
        self.assertEqual(expected, sealed_guard._resolve(
            os.fspath(Path(__file__).resolve().parents[2] / "openatca-data")
        ))
        with patch.dict(os.environ):
            os.environ.pop("OPENATCA_DATA", None)
            with patch.object(sealed_guard, "DEFAULT_DATA_ROOT", self.root):
                guard = sealed_guard.Guard()
            self.assertEqual(guard.root, str(self.root.resolve()))
            with self.assertRaises(sealed_guard.SealedError):
                guard.check(self.root / "derived/public/2026.11.20.synthetic.oaf")

    def test_configured_data_root_override_is_resolved_before_refusal_check(self):
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            data.mkdir()
            (data / ROOT_SENTINEL_FILE).write_bytes(ROOT_SENTINEL_FIXTURE)
            alias = Path(temporary) / "data-link"
            alias.symlink_to(data, target_is_directory=True)
            with patch.dict(os.environ, {"OPENATCA_DATA": str(data)}):
                guard = sealed_guard.Guard()
            self.assertFalse(guard.broken)
            candidate = data / "derived/public/2026.11.20.synthetic.oaf"
            with self.assertRaises(sealed_guard.SealedError):
                guard.check(candidate)
            with patch.dict(os.environ, {"OPENATCA_DATA": str(alias)}):
                linked = sealed_guard.Guard()
            self.assertFalse(linked.broken)
            self.assertEqual(linked.root, str(data.resolve()))
            with self.assertRaises(sealed_guard.SealedError):
                linked.check(candidate)

    def test_empty_configured_data_root_fails_closed(self):
        with patch.dict(os.environ, {"OPENATCA_DATA": ""}):
            guard = sealed_guard.Guard()
            self.assertTrue(guard.broken)
            with self.assertRaises(sealed_guard.SealedError):
                guard.check(Path("/tmp/ordinary.synthetic"))

    def test_relative_configured_data_root_fails_closed(self):
        with patch.dict(os.environ, {"OPENATCA_DATA": "relative-data"}):
            guard = sealed_guard.Guard()
            self.assertTrue(guard.broken)

    def test_wrong_default_root_without_override_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ):
            os.environ.pop("OPENATCA_DATA", None)
            with patch.object(sealed_guard, "DEFAULT_DATA_ROOT", Path(temporary) / "missing"):
                guard = sealed_guard.Guard()
            self.assertTrue(guard.broken)
            self.assertTrue(guard.in_window(0))
            with self.assertRaises(sealed_guard.SealedError):
                guard.check(Path(temporary) / "ordinary.txt")

    def test_configured_roots_require_a_regular_exact_sentinel(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            missing = base / "missing"
            no_sentinel = base / "no-sentinel"
            no_sentinel.mkdir()
            wrong_bytes = base / "wrong-bytes"
            wrong_bytes.mkdir()
            (wrong_bytes / ROOT_SENTINEL_FILE).write_bytes(b"wrong\n")
            directory_sentinel = base / "directory-sentinel"
            directory_sentinel.mkdir()
            (directory_sentinel / ROOT_SENTINEL_FILE).mkdir()
            linked_sentinel = base / "linked-sentinel"
            linked_sentinel.mkdir()
            sentinel_target = base / "sentinel-target"
            sentinel_target.write_bytes(ROOT_SENTINEL_FIXTURE)
            (linked_sentinel / ROOT_SENTINEL_FILE).symlink_to(sentinel_target)
            for bad_root in (missing, no_sentinel, wrong_bytes, directory_sentinel,
                             linked_sentinel, Path("/")):
                with self.subTest(root=bad_root):
                    with patch.dict(os.environ, {"OPENATCA_DATA": str(bad_root)}):
                        guard = sealed_guard.Guard()
                    self.assertTrue(guard.broken)
                    self.assertTrue(guard.in_window(-(1 << 63)))
                    self.assertTrue(guard.in_window((1 << 63) - 1))
                    with self.assertRaises(sealed_guard.SealedError):
                        guard.check(base / "outside-configured-root.txt")

    def test_production_guard_refuses_paths_and_timestamps_for_an_invalid_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            missing = Path(temporary) / "missing-data-root"
            sealed_guard.production.cache_clear()
            try:
                with patch.dict(os.environ, {"OPENATCA_DATA": str(missing)}):
                    guard = sealed_guard.production()
                self.assertTrue(guard.broken)
                self.assertTrue(guard.in_window(-(1 << 63)))
                self.assertTrue(guard.in_window((1 << 63) - 1))
                with self.assertRaises(sealed_guard.SealedError):
                    guard.check(Path(temporary) / "outside-root.txt")
            finally:
                sealed_guard.production.cache_clear()

    def test_valid_sentinel_root_allows_unsealed_and_refuses_sealed_paths(self):
        self.assertFalse(self.g.broken)
        self.assertIsNone(self.g.check(self.root / "ordinary.txt"))
        with self.assertRaises(sealed_guard.SealedError):
            self.g.check(self.root / "scds/rec-20261119/day2/sfdps/a.bin")

    def tearDown(self):
        self._t.cleanup()

    def refused(self, p):
        with self.assertRaises(sealed_guard.SealedError) as c:
            self.g.check(p)
        self.assertIn("OA-SEAL-001", str(c.exception))

    # REQ: OA-SEAL-001
    def test_the_list_parses_to_the_sealed_entries(self):
        root, entries, days, windows = sealed_guard.parse_list(sealed_guard.LIST_PATH.read_text())
        want = ["scds/rec-20261119/day2*", "scds/rec-20261119/day3*", "scds/rec-20261119/day4*",
                "scds/rec-20261119/runs*", "scds/stdds-20261119-holdout*", "scds/stdds-smoke-20261119*"]
        self.assertEqual([e[0] for e in entries], want)
        self.assertEqual(days[0], ["2026-11-20", "2026-11-21", "2026-11-22"])
        self.assertEqual([t[0] for t in days[1]], ["raw", "scds", "derived"])
        self.assertEqual([t[0] for t in days[2]], ["raw/rap", "raw/awc"])
        self.assertEqual([w[0] for w in windows], ["2026-11-20T00:00:00Z 2026-11-23T00:17:00Z"])
        for bad in ("", "root /d\n", "seal a/b\n", "root /d\nseal a/../b\n", "root d\nseal a\n",
                    "root /d\nseal *\n", "root /d\nfoo bar\n", "root /d\nseal **\n", "root /d\nseal a/*x\n",
                    "root /d\nseal a*/b\n", "root /d\nseal a\nwindow 2026-11-20T00:00:00Z\n",
                    "root /d\nseal a\nwindow 2026-11-20T00:00:00Z 2026-11-20T00:00:00Z\n",
                    "root /d\nseal a\nwindow 2026-11-21T00:00:00Z 2026-11-20T00:00:00Z\n",
                    "root /d\nseal a\nwindow 2026-02-29T00:00:00Z 2026-03-01T00:00:00Z\n",
                    "root /d\nseal a\nwindow 2026-11-20 2026-11-21\n"):
            with self.assertRaises(ValueError):
                sealed_guard.parse_list(bad)
        broken = sealed_guard.Guard(text="garbage")
        with self.assertRaises(sealed_guard.SealedError):
            broken.check("/tmp/anything")

    # REQ: OA-SEAL-002
    def test_names_as_given_and_near_misses(self):
        for p in ("x/scds/rec-20261119/day2", "/data/scds/rec-20261119/day2/sub/f", "/data/scds/rec-20261119/runs.log", "/data/scds/rec-20261119/day2x", "/data/scds/rec-20261119/day3.partial", "/data/scds/stdds-20261119-holdout/a",
                  "/data/scds/stdds-smoke-20261119T192936Z/b", "/data/scds/stdds-smoke-20261119"):
            self.refused(p)
        for p in ("/data/scds/day-20261114/rec", "/data/scds/rec-20261119x", "/data/scds/rec-20261119/day1/x",
                  "/data/rec-20261119",
                  "/data/scds/x/rec-20261119", "/data/scds/stdds-smoke-20261118T1"):
            self.g.check(p)

    # REQ: OA-SEAL-010, OA-SEAL-002
    def test_every_sealed_adsb_day_in_both_name_forms_and_derived_clips_and_the_stdds_prefix_are_sealed(self):
        for p in ("/data/raw/adsblol/2026.11.20", "/data/raw/adsblol/2026.11.21/v2026.11.21-planes-readsb-prod-0.tar.aa",
                  "/data/raw/adsblol/2026.11.22/x.oaf", "/data/raw/adsblol/v2026.11.20-x.tar.aa",
                  "/data/raw/adsblol/v2026.11.22-planes-readsb-prod-0.tar.aa", "/data/raw/adsblol/2026.11.21.bak",
                  "/data/derived/adsb/zau-2026.11.21-1s.oaf", "/data/derived/zau-2026.11.20.oaf",
                  "/data/derived/adsb/2026.11.22/zau.oaf", "/data/scds/stdds-20261119-holdout.tar",
                  "/data/scds/stdds-20261119-holdout.bak", "/data/scds/stdds-20261119-holdout-copy/x",
                  "/data/raw/adsblol/dev/2026.11.21/x.oaf", "/data/raw/adsblol/zau-2026.11.20.tar",
                  "/data/raw/adsblol/zau-2026-11-21.tar", "/data/raw/adsblol/zau_2026_11_22.tar",
                  "/data/scds/day-20261120", "/data/scds/rec-20261120", "/data/scds/a/b/c/2026-11-22",
                  "/data/derived/zau-20261121.oaf"):
            self.refused(p)
        for p in ("/data/raw/adsblol/2026.11.24", "/data/raw/adsblol/2026.11.19/x.oaf",
                  "/data/raw/adsblol/v2026.11.24-planes-readsb-prod-0.tar.aa", "/data/raw/adsblol", "/data/derived",
                  "/data/derived/adsb", "/data/derived/adsb/zau-2026.11.14.oaf", "/data/derived/adsb/zau-2026.11.2.oaf",
                  "/data/scds/stdds-20261119", "/data/scds/stdds-2026111",
                  "/data/scds/day-20261124", "/data/scds/rec-20261119/day1", "/data/raw/adsblol/2026.11.14",
                  "/data/raw/rap/rap.20261120.t00z.grib2", "/data/raw/awc/2026-11-21.json"):
            self.g.check(p)

    # REQ: OA-SEAL-010
    def test_a_sealed_day_is_matched_on_normalised_components_split_components_and_every_raw_feed(self):
        for p in ("/data/raw/sfdps/2026.11.20/a.xml", "/data/raw/stdds/2026-11-21.oaf",
                  "/data/raw/tfms/x_2026_11_22_y", "/data/raw/tbfm/20261120.bin",
                  "/data/derived/zau-2026.11_20.oaf", "/data/raw/sfdps/2026/11/20/a.xml",
                  "/data/scds/2026/11/22", "/data/derived/2026-11/21/x", "/data/derived/202611/21/x",
                  "/data/derived/120261120", "/data/scds/1720261120.bin", "/data/raw/adsblol/120261121.tar",
                  "/elsewhere/copy/raw/sfdps/2026.11.21/a"):
            self.refused(p)
        for p in ("/data/raw/rap/2026/11/20/x.grib2", "/data/raw/awc/20261122.json",
                  "/data/raw/sfdps/2026.11.19/a.xml", "/data/raw/sfdps/2026/11/19", "/data/raw/sfdps/2026/12/20",
                  "/data/raw/sfdps/2026/20/11", "/data/derived/2026-11/19/x", "/data/other/2026.11.20/a",
                  "/data/2026.11.20", "/data/raw/sfdps/20.11.2026/a", "/data/raw/sfdps/20-nov-2026/a"):
            self.g.check(p)

    # REQ: OA-SEAL-012
    def test_walk_unsealed_links_back_depth_missing_root_and_order(self):
        w = self.root / "work"
        (w / "a").mkdir(parents=True)
        (w / "a/f.bin").write_bytes(b"x")
        os.symlink(w, w / "a/self")
        files, skipped, again = self.g.walk_unsealed(w)
        self.assertEqual((len(files), skipped, again), (1, 0, 1))
        os.symlink(w, w / "a/self2")
        files, skipped, again = self.g.walk_unsealed(w)
        self.assertEqual((len(files), skipped, again), (1, 0, 2))
        # A sibling link to a directory next to it: the first name in bytewise order is walked,
        # the other is counted as already visited, never as sealed.
        # Eight such pairs, so a walk in the order the directory lists them (not sorted) would reach
        # `real` first in some pair and show it in the names visited.
        sib = self.root / "sib"
        for k in range(8):
            p = sib / f"p{k}"
            (p / "real").mkdir(parents=True)
            (p / "real/f.bin").write_bytes(b"x")
            (p / "a").mkdir()
            os.symlink("../real", p / "a/x")
        files, skipped, again = self.g.walk_unsealed(sib)
        self.assertEqual(([os.path.relpath(f, sib) for f in files], skipped, again),
                         ([f"p{k}/a/x/f.bin" for k in range(8)], 0, 8))
        deep = self.root / "deep"
        for _ in range(70):
            deep = deep / "d"
        deep.mkdir(parents=True)
        (deep / "f").write_bytes(b"x")
        with self.assertRaises(sealed_guard.SealedError):
            self.g.walk_unsealed(self.root / "deep")
        with self.assertRaises(OSError):
            self.g.walk_unsealed(self.root / "nothing-here")
        s = self.root / "sorted"
        (s / "a").mkdir(parents=True)
        (s / "a/b").write_bytes(b"x")
        (s / "a.b").write_bytes(b"x")
        files, _, _ = self.g.walk_unsealed(s)
        self.assertEqual([os.path.relpath(f, s) for f in files], ["a.b", "a/b"])

    # REQ: OA-SEAL-002, OA-SEAL-007
    def test_a_file_with_a_second_hard_link_is_refused_when_opened(self):
        sealed = self.root / "scds/rec-20261119/day2/sfdps/a.bin"
        neutral = self.root / "derived-copy.bin"
        os.link(sealed, neutral)
        with self.assertRaises(sealed_guard.SealedError):
            self.g.open(neutral, "rb")
        one = self.root / "one.bin"
        one.write_bytes(b"x")
        self.g.open(one, "rb").close()

    # REQ: OA-SEAL-011, OA-SEAL-001
    def test_the_window_is_half_open_and_parses_like_the_rust_one(self):
        _, _, _, windows = sealed_guard.parse_list(sealed_guard.LIST_PATH.read_text())
        m = sealed_guard.WINDOW_MARGIN_NS
        self.assertEqual(m, 900_000_000_000)
        self.assertEqual(windows[0][1:], (1_795_132_800_000_000_000 - m, 1_795_393_020_000_000_000 + m))
        lo, hi = windows[0][1:]
        with patch.object(sealed_guard, "production", return_value=self.g):
            for t, sealed in ((lo - 1, False), (lo, True), (hi - 1, True), (hi, False), (0, False)):
                self.assertEqual(self.g.in_window(t), sealed, t)
                self.assertEqual(sealed_guard.in_sealed_window(t), sealed, t)
        self.assertTrue(sealed_guard.Guard(text="garbage").in_window(0))
        two = ("root /d\nseal a\nwindow 2024-02-29T00:00:00Z 2024-03-01T00:00:00Z\n"
               "window 1970-01-01T00:00:01Z 1970-01-01T00:00:02Z\n")
        _, _, _, w2 = sealed_guard.parse_list(two)
        self.assertEqual((w2[0][1], w2[1][1]), (1_709_164_800_000_000_000 - m, 1_000_000_000 - m))
        # A list with no window line seals every instant (fails closed).
        self.assertTrue(sealed_guard.Guard(text="root /d\nseal a\n", root=self.root).in_window(0))
        self.assertTrue(sealed_guard.dump(two).endswith("window 1970-01-01T00:00:01Z 1970-01-01T00:00:02Z\n"))

    # REQ: OA-SEAL-014
    def test_a_list_that_seals_every_instant_warns_at_start_and_a_good_list_does_not(self):
        import contextlib
        import io
        from unittest import mock

        nowin = "root /d\nseal a\n"
        self.assertEqual(sealed_guard.start_warning(sealed_guard.Guard(text=nowin, root=self.root)),
                         "seal list has no window: every timed row is dropped")
        self.assertEqual(sealed_guard.start_warning(sealed_guard.Guard(text="garbage")),
                         "seal list does not parse: every timed row is dropped and every sealed path refused")
        self.assertIsNone(sealed_guard.start_warning(self.g))
        # production() prints it, once, on stderr; the cache is bypassed for the test.
        for text, line in ((nowin, "seal list has no window: every timed row is dropped\n"),
                           ("garbage", "seal list does not parse: every timed row is dropped"
                                       " and every sealed path refused\n"),
                           (None, "")):
            err = io.StringIO()
            real = sealed_guard.Guard
            guard_patch = (mock.patch.object(sealed_guard, "Guard", lambda: real(text=text, root=self.root))
                           if text is not None else mock.patch.object(sealed_guard, "DEFAULT_DATA_ROOT", self.root))
            with guard_patch:
                with contextlib.redirect_stderr(err):
                    sealed_guard.production.__wrapped__()
            self.assertEqual(err.getvalue(), line)

    # REQ: OA-SEAL-012, OA-SEAL-003
    def test_walk_unsealed_skips_and_counts_sealed_entries(self):
        adsb = self.root / "raw/adsblol"
        for d in ("2026.11.19", "2026.11.20", "2026.11.24"):
            (adsb / d).mkdir(parents=True)
            (adsb / d / "a.oaf").write_bytes(b"x")
        (adsb / "v2026.11.21-x.tar.aa").write_bytes(b"x")
        (adsb / "v2026.11.23-x.tar.aa").write_bytes(b"x")
        files, skipped, _ = self.g.walk_unsealed(adsb)
        self.assertEqual([os.path.relpath(f, adsb) for f in files],
                         ["2026.11.19/a.oaf", "2026.11.24/a.oaf", "v2026.11.23-x.tar.aa"])
        self.assertEqual(skipped, 2)
        os.symlink(self.root / "scds/rec-20261119/day2", adsb / "2026.11.30")
        files, skipped, _ = self.g.walk_unsealed(adsb)
        self.assertEqual((len(files), skipped), (3, 3))
        with self.assertRaises(sealed_guard.SealedError):
            self.g.walk_unsealed(adsb / "2026.11.20")
        # The data root holds sealed entries: it is walked, and they are skipped and counted.
        files, skipped, _ = self.g.walk_unsealed(self.root)
        self.assertGreaterEqual(skipped, 6)
        self.assertIn(str(self.root / "scds/day-20261118/rec/b.bin"), files)
        for f in files:
            self.g.check(f)

    # REQ: OA-SEAL-009, OA-SEAL-001, OA-SEAL-002
    def test_only_days_two_to_four_and_the_run_log_of_the_recording_are_sealed(self):
        rec = self.root / "scds/rec-20261119"
        (rec / "day1/x").write_bytes(b"x")
        (rec / "day3").mkdir()
        (rec / "day4").mkdir()
        (rec / "bin/run-days.sh").write_bytes(b"x")
        for p in ("day1/x", "bin/run-days.sh", "bin", "day1", "day1x", "day5"):
            self.g.check(rec / p)
        # The folder itself holds sealed entries, so it is refused as a holding directory (as
        # `scds` is): a tool names day1 or bin directly.
        with self.assertRaises(sealed_guard.SealedError) as c:
            self.g.check(rec)
        self.assertIn("holds", str(c.exception))
        for p in ("day2/x", "day3", "day3/x", "day4/x", "runs.log", "runs.log.1", "day2.tmp", "day30", "day1/../day2/x"):
            self.refused(rec / p)
        work = self.base / "work"
        work.mkdir()
        os.symlink(rec / "day2/sfdps/a.bin", rec / "day1/link")
        self.refused(rec / "day1/link")
        os.symlink(rec / "day2", work / "day1")
        self.refused(work / "day1")
        self.refused(work / "day1/sfdps/a.bin")
        self.refused("/elsewhere/scds/rec-20261119/day2/x")
        self.refused("/elsewhere/scds/rec-20261119/runs.log")
        self.g.check("/elsewhere/scds/rec-20261119/day1/x")

    # REQ: OA-SEAL-002
    def test_paths_are_resolved_before_the_check(self):
        work = self.base / "work"
        work.mkdir()
        sealed = self.root / "scds/rec-20261119/day2"
        os.symlink(sealed, work / "dir")
        os.symlink(sealed / "sfdps/a.bin", work / "file")
        os.symlink(work / "dir", work / "chain")
        for p in (work / "dir", work / "dir/sfdps/a.bin", work / "file", work / "chain/sfdps",
                  self.root / "scds/day-20261118/../rec-20261119/day2/sfdps",
                  work / "dir/new-output", work / "dir/not/yet/here",
                  self.root / "scds/day-20261118/nothing/../../rec-20261119/day2"):
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
        os.symlink(self.root / "scds/rec-20261119/day2/sfdps/a.bin", dev / "sneaky")
        read, refused = 0, 0
        for p in sorted(dev.iterdir()):
            try:
                with self.g.open(p, "rb"):
                    read += 1
            except sealed_guard.SealedError:
                refused += 1
        self.assertEqual((read, refused), (1, 1))

    # REQ: OA-SEAL-004
    def test_no_disable_switch_is_accepted(self):
        src = (Path(sealed_guard.__file__)).read_text()
        for word in ("OPENATCA_SEAL_DISABLE", "OPENATCA_SEAL_BYPASS", "argparse"):
            self.assertNotIn(word, src)
        r = subprocess.run([sys.executable, "-I", sealed_guard.__file__, "--off", "x"],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        sealed_dir = self.root / "scds/rec-20261119/day2/sfdps/a.bin"
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
                            env=dict(os.environ, OPENATCA_DATA=str(self.root)),
                            capture_output=True, text=True)
        self.assertEqual(ok.returncode, 0)

    # REQ: OA-SEAL-005
    def test_the_shell_scripts_refuse_a_sealed_path_before_anything_is_created(self):
        here = Path(__file__).resolve().parent
        out = self.base / "out"
        # Fake paths that only name a sealed folder: nothing real is touched.
        r = subprocess.run(["bash", str(here / "replay_compare.sh"), "HEAD", "HEAD",
                            "/nonexistent-fake/scds/rec-20261119/day2/clip.oaf", str(out)],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr)
        self.assertFalse(out.exists())
        # An output directory under a link to a sealed folder is refused as well.
        os.symlink(self.root / "scds/rec-20261119/day2", self.base / "lnk")
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
        os.symlink(self.root / "scds/rec-20261119/day2/sfdps/a.bin", self.base / "late")
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
        sealed = t / "scds" / "rec-20261119" / "day2" / "x"
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
                for link in (open_dir / "file-link", open_dir / "dir-link", open_dir / "parent-link" / "rec-20261119" / "day2" / "x" / "f.csv"):
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

    # REQ: OA-SEAL-012, OA-SEAL-003
    def test_make_corpora_skips_sealed_directories_and_reads_through_the_guard(self):
        import gzip
        import io
        import shutil
        import tarfile
        here = Path(__file__).resolve().parent
        repo = here.parent
        copy = self.base / "repo"
        (copy / "fuzz").mkdir(parents=True)
        (copy / "tools").mkdir()
        (copy / "crates/oa-sealed").mkdir(parents=True)
        shutil.copy(repo / "fuzz/make_corpora.py", copy / "fuzz")
        shutil.copy(here / "sealed_guard.py", copy / "tools")
        shutil.copy(repo / "crates/oa-sealed/sealed_paths.txt", copy / "crates/oa-sealed")
        shutil.copytree(repo / "crates/oa-arinc424/tests/fixtures", copy / "crates/oa-arinc424/tests/fixtures")
        (copy / "crates/oa-notam/tests/common").mkdir(parents=True)
        shutil.copy(repo / "crates/oa-notam/tests/common/mod.rs", copy / "crates/oa-notam/tests/common")
        data = self.base / "mkdata"
        data.mkdir()
        (data / ROOT_SENTINEL_FILE).write_bytes(ROOT_SENTINEL_FIXTURE)
        for f in ("cat048.raw", "cat062cat065.raw", "cat21_re.ast"):
            (data / "raw/asterix-samples").mkdir(parents=True, exist_ok=True)
            (data / "raw/asterix-samples" / f).write_bytes(b"")

        (data / "raw/nasr-2026-10-29/x").mkdir(parents=True)
        for name in ("STAR_BASE", "STAR_APT", "STAR_RTE", "DP_BASE", "DP_APT", "DP_RTE", "AWY_BASE", "AWY_SEG_ALT"):
            (data / f"raw/nasr-2026-10-29/x/{name}.csv").write_bytes(b"A,B\n1,2\n")

        def part(day, name):
            bio = io.BytesIO()
            with tarfile.open(fileobj=bio, mode="w") as t:
                body = gzip.compress(b'{"day": "%s"}' % name.encode())
                ti = tarfile.TarInfo(f"traces/{name}.json")
                ti.size = len(body)
                t.addfile(ti, io.BytesIO(body))
            d = data / "raw/adsblol" / day
            d.mkdir(parents=True)
            (d / f"v{day}-planes-readsb-prod-0.tar.aa").write_bytes(bio.getvalue())

        part("2026.11.19", "open-day")
        part("2026.11.21", "sealed-day")
        env = dict(os.environ, OPENATCA_DATA=str(data))
        r = subprocess.run([sys.executable, "-I", str(copy / "fuzz/make_corpora.py")], env=env,
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("traces 1", r.stdout)
        self.assertIn("sealed entries skipped: 1", r.stdout)
        corpus = b"".join(f.read_bytes() for f in (data / "fuzz/adsb-trace/corpus").iterdir())
        self.assertIn(b"open-day", corpus)
        self.assertNotIn(b"sealed-day", corpus)
        # Every read goes through the guard: no bare open of an input remains in the script.
        src = (repo / "fuzz/make_corpora.py").read_text()
        self.assertNotIn("'rb').read()", src)
        self.assertNotIn("glob.glob(", src.replace("for p in glob.glob(pattern):", ""))
        self.assertIn("sealed_guard.open_read", src)

    # REQ: OA-SEAL-008
    def test_odd_list_files_are_refused_by_the_python_parser(self):
        for bad in ("root /d\nseal a\tb\n", "root /d\r\nseal a\n", "root  /d\nseal a\n", "root /d\nseal a \n",
                    "root /d\n seal a\n", "root /d\nseal \x00a\n", "root /d\nseal a\x0bb\n", "root /d\nseal a\x1cb\n",
                    "root /d\nseal \u00e9\n", "root /d\nseal a\u2028b\n", "root /d\n \nseal a\n", "root /d\nseal a//b\n"):
            with self.assertRaises(ValueError, msg=repr(bad)):
                sealed_guard.parse_list(bad)
        sealed_guard.parse_list("# any printable ~!@ in a comment\nroot /d\nseal a/b*")
        sealed_guard.parse_list("root /d\nseal a/*b*\nwindow 2026-11-20T00:00:00Z 2026-11-21T00:00:00Z")
        with self.assertRaises(ValueError):
            sealed_guard.parse_list("root $OTHER_ROOT\nseal a\n")
        sealed_guard.parse_list("root $OPENATCA_DATA\nseal a\n")


if __name__ == "__main__":
    unittest.main()
