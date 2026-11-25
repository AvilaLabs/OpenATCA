"""Synthetic checks for descriptor-verified I/O used by the Chicago package scripts."""
import hashlib
import lzma
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

IO = Path(__file__).resolve().with_name("repro_package_io.py")
sys.path.insert(0, str(IO.parent))
import repro_package_io


class PackageIoTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def run_io(self, *args):
        return subprocess.run(
            [sys.executable, "-I", str(IO), *map(str, args)],
            capture_output=True,
            timeout=30,
        )

    def sealed_file(self):
        target = self.root / "scds" / "rec-20261119" / "day2" / "synthetic.bin"
        target.parent.mkdir(parents=True)
        target.write_bytes(b"sealed synthetic bytes")
        return target

    def test_measure_and_read_use_the_opened_descriptor(self):
        data = b"synthetic development package bytes\n"
        source = self.root / "input.bin"
        source.write_bytes(data)
        measured = self.run_io("measure", source)
        self.assertEqual(measured.returncode, 0, measured.stderr)
        self.assertEqual(measured.stdout.decode().strip(), f"{len(data)} {hashlib.sha256(data).hexdigest()}")
        read = self.run_io("read", source)
        self.assertEqual((read.returncode, read.stdout), (0, data))

    def test_sealed_symlink_is_rejected_before_bytes_are_returned(self):
        target = self.sealed_file()
        link = self.root / "neutral-name.bin"
        link.symlink_to(target)
        result = self.run_io("read", link)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"OA-SEAL-001", result.stderr)
        self.assertEqual(result.stdout, b"")

    def test_missing_is_distinguished_from_refused_and_measured(self):
        missing = self.root / "absent.bin"
        present = self.run_io("present", missing)
        self.assertEqual(present.returncode, 3)
        missing.write_bytes(b"now present")
        present = self.run_io("present", missing)
        self.assertEqual(present.returncode, 0, present.stderr)

    def test_xz_unpack_uses_guarded_input_and_verifies_plain_size_and_digest(self):
        plain = b"synthetic plain export\n"
        compressed = self.root / "export.xz"
        compressed.write_bytes(lzma.compress(plain))
        output = self.root / "out" / "export.txt"
        output.parent.mkdir()
        args = ("unpack-xz", compressed, output, len(plain), hashlib.sha256(plain).hexdigest())
        result = self.run_io(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.read_bytes(), plain)
        bad_output = output.parent / "bad.txt"
        result = self.run_io("unpack-xz", compressed, bad_output, len(plain), "0" * 64)
        self.assertEqual(result.returncode, 2)
        self.assertFalse(bad_output.exists())
        result = self.run_io("unpack-xz", compressed, bad_output, len(plain) - 1, args[-1])
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"size exceeds", result.stderr)
        self.assertFalse(bad_output.exists())

    def test_xz_unpack_refuses_truncation_trailing_members_and_low_memory_without_partial(self):
        plain = b"synthetic bounded export\n"
        compressed = self.root / "single.xz"
        good = lzma.compress(plain)
        output = self.root / "out" / "strict.txt"
        output.parent.mkdir()
        sha = hashlib.sha256(plain).hexdigest()
        for payload in (good[:-3], good + lzma.compress(b"second member"), good + b"trailing"):
            compressed.write_bytes(payload)
            result = self.run_io("unpack-xz", compressed, output, len(plain), sha)
            self.assertEqual(result.returncode, 2)
            self.assertFalse(output.exists())
        compressed.write_bytes(good)
        with patch.object(repro_package_io, "MAX_XZ_MEMORY_BYTES", 1):
            with self.assertRaises(lzma.LZMAError):
                repro_package_io._unpack(str(compressed), str(output), len(plain), sha)
        self.assertFalse(output.exists())

    def test_non_regular_inputs_are_refused(self):
        fifo = self.root / "fifo"
        os.mkfifo(fifo)
        result = self.run_io("present", fifo)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"expected a regular file", result.stderr)

    def test_masked_hash_matches_the_previous_output_normalization(self):
        data = (
            b"scorer binary sha256: " + b"a" * 64 + b"\n"
            b"CLAIM run binary=" + b"b" * 64 + b"\n"
        )
        source = self.root / "p002.txt"
        source.write_bytes(data)
        normalized = (
            b"scorer binary sha256: BINARY\n"
            b"CLAIM run binary=BINARY\n"
        )
        result = self.run_io("masked-hash", source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.decode().strip(), hashlib.sha256(normalized).hexdigest())

    def test_manifest_bounds_duplicates_and_clip_option_allowlist(self):
        sha = "a" * 64
        manifest = (
            "reproduce-zau-manifest 1\n"
            "day 2026-11-15\ncommit " + "0" * 40 + "\nlabel synthetic\n"
            "adsb-release https://github.com/adsblol/globe_history_2026/releases/tag/v2026.11.15-planes-readsb-prod-0\n"
            f"adsb-file v2026.11.15-planes-readsb-prod-0.tar.aa 1 {sha} "
            "https://github.com/adsblol/globe_history_2026/releases/download/"
            "v2026.11.15-planes-readsb-prod-0/v2026.11.15-planes-readsb-prod-0.tar.aa\n"
            "clip-args --frame-ms 1000\nclip clip.oaf 1 " + sha + "\n"
            "eram-export eram.txt 1 " + sha + "\n"
            "output p002.txt " + sha + " " + sha + "\noutput samples.csv " + sha + "\n"
        )
        path = self.root / "MANIFEST"
        path.write_text(manifest)
        self.assertEqual(self.run_io("validate-manifest", path).returncode, 0)
        for altered in (
            manifest + "label duplicate\n",
            manifest + "unknown value\n",
            manifest.replace("--frame-ms 1000", "--allow-truncated"),
            manifest.replace("v2026.11.15-planes-readsb-prod-0.tar.aa", ".."),
            manifest.replace("v2026.11.15-planes-readsb-prod-0.tar.aa", "eram.txt.sha256"),
            manifest.replace(
                "https://github.com/adsblol/globe_history_2026/releases/download/",
                "https://example.invalid/releases/download/",
            ),
            manifest.replace("label synthetic", "label bad\x01label"),
            manifest + ("#" + "x" * (64 * 1024) + "\n"),
        ):
            path.write_text(altered)
            self.assertEqual(self.run_io("validate-manifest", path).returncode, 2)


if __name__ == "__main__":
    unittest.main()
