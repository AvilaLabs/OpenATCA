"""The refusals and input checks of reproduce_zau.sh and the manifest zau_manifest.sh writes. No
build is started and no real data is read: the package is made of small fake files, and the commit
the manifest names is not the checkout's, so every run stops before the build."""

import hashlib
import subprocess
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
REPRO = TOOLS / "reproduce_zau.sh"
WRITER = TOOLS / "zau_manifest.sh"
DAY = "2026-11-15"
FAKE_COMMIT = "0" * 40


def sh(script, *args):
    return subprocess.run(
        ["bash", str(script), *map(str, args)], capture_output=True, text=True, timeout=60
    )


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


class Package:
    """A fake package under a temp directory: export, two archive parts, a reference run."""

    def __init__(self, base: Path):
        self.pkg = base / "pkg"
        self.pkg.mkdir()
        self.run = base / "run"
        self.run.mkdir()
        self.out = base / "out"
        (self.pkg / "eram.txt").write_bytes(b"export\n")
        (self.pkg / "eram.txt.sha256").write_text(f"{sha(b'export' + chr(10).encode())}  eram.txt\n")
        self.parts = []
        for n, body in (("v2026.11.15-planes-readsb-prod-0.tar.aa", b"aa" * 5),
                        ("v2026.11.15-planes-readsb-prod-0.tar.ab", b"bb" * 3)):
            p = base / n
            p.write_bytes(body)
            self.parts.append(p)
        self.clip = base / "zau-2026.11.15.oaf"
        self.clip.write_bytes(b"clip")
        (self.run / "p002.txt").write_text("a\n  scorer binary sha256: " + "a" * 64 + "; x\nCLAIM run binary=" + "b" * 64 + " y\n")
        (self.run / "samples.csv").write_text("csv\n")
        # The parts sit in the package directory too, as a downloader would leave them.
        for p in self.parts:
            (self.pkg / p.name).write_bytes(p.read_bytes())

    def manifest(self, commit=FAKE_COMMIT, day=DAY):
        r = sh(WRITER, day, commit, self.pkg, "eram.txt", self.clip, "--frame-ms 1000", self.run,
               "a label", *self.parts)
        assert r.returncode == 0, r.stderr
        return (self.pkg / "MANIFEST").read_text()


class ReproduceTests(unittest.TestCase):
    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.base = Path(self._t.name).resolve()
        self.p = Package(self.base)

    def tearDown(self):
        self._t.cleanup()

    def sealed_fixture(self, name="fixture.bin"):
        path = self.base / "scds" / "rec-20261119" / "day2" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"sealed synthetic fixture")
        return path

    # REQ: OA-REP-009, OA-REP-010
    def test_writer_refuses_multiline_label_or_clip_args_before_manifest_publication(self):
        for label, clip_args in (("safe\ncommit " + FAKE_COMMIT, "--frame-ms 1000"),
                                 ("safe", "--frame-ms 1000\nadsb-file injected 1 " + "a" * 64 + " url")):
            result = sh(WRITER, DAY, FAKE_COMMIT, self.p.pkg, "eram.txt", self.p.clip,
                        clip_args, self.p.run, label, *self.p.parts)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertFalse((self.p.pkg / "MANIFEST").exists())
            self.assertFalse((self.p.pkg / "MANIFEST.partial").exists())

    # REQ: OA-REP-008
    def test_the_writer_makes_a_manifest_the_script_reads_to_the_commit_check(self):
        m = self.p.manifest()
        self.assertIn(f"commit {FAKE_COMMIT}", m)
        self.assertIn(f"eram-export eram.txt 7 {sha(b'export' + chr(10).encode())}", m)
        self.assertIn("adsb-file v2026.11.15-planes-readsb-prod-0.tar.aa 10 " + sha(b"aa" * 5), m)
        self.assertIn("releases/download/v2026.11.15-planes-readsb-prod-0/v2026.11.15-planes-readsb-prod-0.tar.ab", m)
        self.assertIn("clip-args --frame-ms 1000", m)
        # The masked hash is of the text with both binary hashes replaced.
        masked = "a\n  scorer binary sha256: BINARY; x\nCLAIM run binary=BINARY y\n"
        self.assertIn(f"output p002.txt {sha((self.p.run / 'p002.txt').read_bytes())} {sha(masked.encode())}", m)
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("input ok: eram.txt", r.stdout)
        self.assertIn("HEAD is", r.stderr)
        self.assertFalse(self.p.out.exists(), "nothing is created before the checks pass")

    # REQ: OA-REP-008
    def test_an_export_published_only_packed_is_checked_by_its_packed_hash(self):
        subprocess.run(["xz", "-k", str(self.p.pkg / "eram.txt")], check=True)
        m = self.p.manifest()
        self.assertIn("eram-export-xz eram.txt.xz ", m)
        (self.p.pkg / "eram.txt").unlink()
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("input ok: eram.txt.xz", r.stdout)
        self.assertIn("HEAD is", r.stderr)
        (self.p.pkg / "eram.txt.xz").write_bytes(b"not xz")
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2)
        self.assertIn("eram.txt.xz", r.stderr)

    # REQ: OA-REP-008, OA-SEAL-005
    def test_usage_and_the_sealed_days_and_paths_are_refused(self):
        self.p.manifest()
        self.assertEqual(sh(REPRO, DAY, self.p.pkg).returncode, 2)
        for day in ("2026-11-20", "2026-11-21", "2026-11-22"):
            r = sh(REPRO, day, self.p.pkg, self.p.out)
            self.assertEqual(r.returncode, 2, day)
            self.assertIn("sealed", r.stderr)
        self.assertEqual(sh(REPRO, "15 Nov", self.p.pkg, self.p.out).returncode, 2)
        sealed = self.base / "scds" / "rec-20261119" / "day2" / "pkg"
        sealed.mkdir(parents=True)
        r = sh(REPRO, DAY, sealed, self.p.out)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr + r.stdout)
        r = sh(REPRO, DAY, self.p.pkg, sealed / "out")
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr + r.stdout)

    # REQ: OA-REP-008
    def test_a_changed_input_a_wrong_manifest_or_a_used_out_dir_is_refused_naming_the_file(self):
        self.p.manifest()
        (self.p.pkg / "eram.txt").write_bytes(b"EXPORT\n")
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2)
        self.assertIn("eram.txt: SHA-256", r.stderr)
        (self.p.pkg / "eram.txt").write_bytes(b"export\n")
        (self.p.pkg / "v2026.11.15-planes-readsb-prod-0.tar.ab").write_bytes(b"b")
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2)
        self.assertIn("tar.ab has size 1", r.stderr)
        (self.p.pkg / "v2026.11.15-planes-readsb-prod-0.tar.ab").unlink()
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertIn("tar.ab is not in", r.stderr)
        self.assertEqual(r.returncode, 2)
        wrong_day_manifest = self.p.manifest()
        wrong_day_manifest = wrong_day_manifest.replace("2026-11-15", "2026-11-19")
        wrong_day_manifest = wrong_day_manifest.replace("v2026.11.15", "v2026.11.19")
        (self.p.pkg / "MANIFEST").write_text(wrong_day_manifest)
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertIn("the MANIFEST is for 2026-11-19", r.stderr)
        self.assertEqual(r.returncode, 2)
        text = (self.p.pkg / "MANIFEST").read_text()
        (self.p.pkg / "MANIFEST").write_text(text.replace("reproduce-zau-manifest 1", "reproduce-zau-manifest 2"))
        self.assertEqual(sh(REPRO, "2026-11-19", self.p.pkg, self.p.out).returncode, 2)
        (self.p.pkg / "MANIFEST").write_text(text + "extra line\n")
        r = sh(REPRO, "2026-11-19", self.p.pkg, self.p.out)
        self.assertIn("MANIFEST line", r.stderr)
        self.assertIn("unknown or empty field", r.stderr)
        (self.p.pkg / "MANIFEST").write_text(text.replace("output samples.csv", "output other.csv"))
        r = sh(REPRO, "2026-11-19", self.p.pkg, self.p.out)
        self.assertIn("unknown or repeated output", r.stderr)
        (self.p.pkg / "MANIFEST").unlink()
        self.assertEqual(sh(REPRO, DAY, self.p.pkg, self.p.out).returncode, 2)
        self.p.manifest()
        self.p.out.mkdir()
        (self.p.out / "old").write_text("x")
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2)
        self.assertIn("new or empty", r.stderr)

    # REQ: OA-REP-008
    def test_the_writer_refuses_a_sealed_day_and_a_bad_commit(self):
        for day, commit in (("2026-11-21", FAKE_COMMIT), (DAY, "abc")):
            r = sh(WRITER, day, commit, self.p.pkg, "eram.txt", self.p.clip, "", self.p.run, "l", *self.p.parts)
            self.assertEqual(r.returncode, 2, (day, commit))
        self.assertFalse((self.p.pkg / "MANIFEST").exists())

    # REQ: OA-REP-008
    def test_writer_links_each_part_to_the_release_derived_from_its_own_stem(self):
        mixed = self.base / "v2026.11.15-planes-readsb-prod-1.tar.aa"
        mixed.write_bytes(b"cc" * 4)
        (self.p.pkg / mixed.name).write_bytes(mixed.read_bytes())
        self.p.parts.append(mixed)
        manifest = self.p.manifest()
        self.assertIn(
            "adsb-file v2026.11.15-planes-readsb-prod-0.tar.aa 10 "
            + sha(b"aa" * 5)
            + " https://github.com/adsblol/globe_history_2026/releases/download/"
            "v2026.11.15-planes-readsb-prod-0/v2026.11.15-planes-readsb-prod-0.tar.aa",
            manifest,
        )
        self.assertIn("adsb-release https://github.com/adsblol/globe_history_2026/releases/tag/v2026.11.15-planes-readsb-prod-0", manifest)
        self.assertEqual(sum(line.startswith("adsb-release ") for line in manifest.splitlines()), 1)
        self.assertIn(
            "adsb-file v2026.11.15-planes-readsb-prod-1.tar.aa 8 "
            + sha(b"cc" * 4)
            + " https://github.com/adsblol/globe_history_2026/releases/download/"
            "v2026.11.15-planes-readsb-prod-1/v2026.11.15-planes-readsb-prod-1.tar.aa",
            manifest,
        )
        self.assertNotIn("adsb-release https://github.com/adsblol/globe_history_2026/releases/tag/v2026.11.15-planes-readsb-prod-1", manifest)

    # REQ: OA-REP-008
    def test_writer_refuses_part_names_without_a_public_release_stem(self):
        part = self.base / "archive.bin"
        part.write_bytes(b"archive")
        r = sh(
            WRITER,
            DAY,
            FAKE_COMMIT,
            self.p.pkg,
            "eram.txt",
            self.p.clip,
            "--frame-ms 1000",
            self.p.run,
            "label",
            part,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("cannot derive adsb.lol release tag", r.stderr)
        self.assertFalse((self.p.pkg / "MANIFEST").exists())

    # REQ: OA-REP-008, OA-REP-009
    def test_writer_refuses_other_days_and_invalid_calendar_dates_in_adsb_part_names(self):
        for name in (
            "v2026.11.16-planes-readsb-prod-0.tar.aa",
            "v2026.02.30-planes-readsb-prod-0.tar.aa",
            "v2027.11.15-planes-readsb-prod-0.tar.aa",
            "archive-prod-0.tar.aa",
        ):
            part = self.base / name
            part.write_bytes(b"archive")
            result = sh(WRITER, DAY, FAKE_COMMIT, self.p.pkg, "eram.txt", self.p.clip,
                        "--frame-ms 1000", self.p.run, "label", part)
            self.assertEqual(result.returncode, 2, name)
            self.assertFalse((self.p.pkg / "MANIFEST").exists())
            self.assertFalse((self.p.pkg / "MANIFEST.partial").exists())

    # REQ: OA-REP-008, OA-REP-009
    def test_reader_binds_each_archive_url_to_the_exact_release_and_filename(self):
        manifest = self.p.manifest()
        wrong = manifest.replace(
            "releases/download/v2026.11.15-planes-readsb-prod-0/",
            "releases/download/v2026.11.15-planes-readsb-prod-1/",
            1,
        )
        (self.p.pkg / "MANIFEST").write_text(wrong)
        result = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(result.returncode, 2)
        self.assertIn("archive part URL does not match its exact release tag and name", result.stderr)
        self.assertFalse(self.p.out.exists())

    # REQ: OA-REP-008, OA-REP-009
    def test_reader_refuses_repeated_release_summary_scalar(self):
        manifest = self.p.manifest()
        first = next(line for line in manifest.splitlines() if line.startswith("adsb-release "))
        (self.p.pkg / "MANIFEST").write_text(manifest + first + "\n")
        result = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(result.returncode, 2)
        self.assertIn("repeats scalar field adsb-release", result.stderr)

    # REQ: OA-REP-008, OA-SEAL-005
    def test_reproducer_checks_manifest_and_each_concrete_input_path(self):
        sealed = self.sealed_fixture()
        self.p.manifest()
        manifest = self.p.pkg / "MANIFEST"
        manifest.unlink()
        manifest.symlink_to(sealed)
        r = sh(REPRO, DAY, self.p.pkg, self.p.out)
        self.assertEqual(r.returncode, 2)
        self.assertIn("OA-SEAL-001", r.stderr + r.stdout)

        manifest.unlink()
        self.p.manifest()
        for rel in ("eram.txt", "eram.txt.sha256", self.p.parts[0].name):
            target = self.sealed_fixture(rel + "-target")
            entry = self.p.pkg / rel
            original = entry.read_bytes()
            entry.unlink()
            entry.symlink_to(target)
            r = sh(REPRO, DAY, self.p.pkg, self.p.out)
            self.assertEqual(r.returncode, 2, rel)
            self.assertIn("OA-SEAL-001", r.stderr + r.stdout, rel)
            self.assertEqual(target.read_bytes(), b"sealed synthetic fixture")
            entry.unlink()
            entry.write_bytes(original)

    # REQ: OA-REP-008, OA-SEAL-005
    def test_manifest_writer_checks_export_and_sidecar_paths(self):
        for rel in ("eram.txt", "eram.txt.sha256", "eram.txt.xz"):
            target = self.sealed_fixture(rel + "-target")
            entry = self.p.pkg / rel
            original = entry.read_bytes() if entry.exists() else None
            if entry.exists() or entry.is_symlink():
                entry.unlink()
            entry.symlink_to(target)
            r = sh(WRITER, DAY, FAKE_COMMIT, self.p.pkg, "eram.txt", self.p.clip,
                   "--frame-ms 1000", self.p.run, "label", *self.p.parts)
            self.assertEqual(r.returncode, 2, rel)
            self.assertIn("OA-SEAL-001", r.stderr + r.stdout, rel)
            self.assertEqual(target.read_bytes(), b"sealed synthetic fixture")
            entry.unlink()
            if original is not None:
                entry.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
