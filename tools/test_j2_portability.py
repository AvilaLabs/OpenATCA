"""Synthetic-only checks for the private ZAU regional replay portability bundle."""
from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock

TOOL = Path(__file__).with_name("j2_portability.py")
SPEC = importlib.util.spec_from_file_location("j2_portability", TOOL)
assert SPEC is not None and SPEC.loader is not None
j2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(j2)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class SyntheticPortabilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="oa-j2-portability-")
        self._populate()

    def _populate(self) -> None:
        self.root = Path(self.temp.name) / "openatca-private"
        self.root.mkdir(mode=0o700)
        self.recording = self.root / "import"
        self.adaptation = self.root / "adaptation"
        self.baseline = self.root / "baseline"
        self.recording.mkdir()
        self.adaptation.mkdir()
        self.baseline.mkdir()
        self.input = b"OA-RS\t1\nv1\t1000000000\t1000000000\t1\tSFDPS\t0\t1\tZAU1\tPOS\t419000000\t-879000000\t10000\t0\t0\t0\n"
        self.provenance = b"OA_PROVENANCE\t1\nsynthetic\n"
        self.quarantine = b"OA_QUARANTINE\t1\n"
        self.counts = b"OA_RECORDING_IMPORT_REPORT\t1\n"
        self.index = (
            "OA_RECORDING_IMPORT_MANIFEST\t1\n"
            "centre\tZAU1\n"
            f"normalized_bytes\t{len(self.input)}\n"
            f"normalized_sha256\t{sha(self.input)}\n"
            f"provenance_bytes\t{len(self.provenance)}\n"
            f"provenance_sha256\t{sha(self.provenance)}\n"
            f"quarantine_bytes\t{len(self.quarantine)}\n"
            f"quarantine_sha256\t{sha(self.quarantine)}\n"
            f"report_bytes\t{len(self.counts)}\n"
            f"report_sha256\t{sha(self.counts)}\n"
            "input_stream\t0\tSFDPS\t" + "a" * 64 + "\t1\t1\t0\n"
        ).encode()
        self._write(self.recording / "normalized.tsv", self.input)
        self._write(self.recording / "provenance.tsv", self.provenance)
        self._write(self.recording / "quarantine.tsv", self.quarantine)
        self._write(self.recording / "counts.tsv", self.counts)
        self._write(self.recording / "manifest.tsv", self.index)
        self._write(self.recording / "COMPLETE", (sha(self.index) + "\n").encode())

        self.snapshot = b"OATCA-ADAPT" + struct.pack("<IIIII", 3, 20261029, 20261029, 50000, 3) + b"ZAU" + b"synthetic snapshot body"
        self.adapt_manifest = (
            b"OAG2ADPT" + struct.pack("<HHH", 1, 1, 1) + bytes((2, 3, 2, 1))
            + struct.pack("<Q", len(self.snapshot)) + bytes.fromhex(sha(self.snapshot)) + b"r1ZAUc1"
        )
        self._write(self.adaptation / "airspace.snapshot", self.snapshot)
        self._write(self.adaptation / "manifest.oag2", self.adapt_manifest)

        self.settings = b"OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\nmodel\tSUPPLIED_SNAPSHOT\n"
        frame_text = b"synthetic frame evidence\n"
        frame_ppm = b"P6\n800 600\n255\n" + bytes(800 * 600 * 3)
        self._write(self.baseline / "settings.tsv", self.settings)
        self._write(self.baseline / "frame-0001.txt", frame_text)
        self._write(self.baseline / "frame-0001.ppm", frame_ppm)
        self.run_manifest = (
            "OA_REGIONAL_MANIFEST\t1\n"
            f"input_sha256\t{sha(self.input)}\n"
            f"snapshot_sha256\t{sha(self.snapshot)}\n"
            f"executable_sha256\t{'b' * 64}\n"
            f"settings_sha256\t{sha(self.settings)}\n"
            f"frame\t1\t1000000000\tframe-0001.txt\t{sha(frame_text)}\t{sha(frame_ppm)}\n"
        ).encode()
        self._write(self.baseline / "manifest.tsv", self.run_manifest)
        self._write(self.baseline / "COMPLETE", (sha(self.run_manifest) + "\n").encode())

    def _reset(self) -> None:
        self.temp.cleanup()
        self.temp = tempfile.TemporaryDirectory(prefix="oa-j2-portability-")
        self._populate()

    def tearDown(self) -> None:
        self.temp.cleanup()

    @staticmethod
    def _write(path: Path, data: bytes) -> None:
        path.write_bytes(data)

    def _export(self, name: str = "bundle") -> Path:
        out = self.root / name
        j2.export_bundle(self.recording, self.adaptation, self.baseline,
                         "1" * 40, out)
        return out

    def test_export_and_fresh_reimport_preserve_all_setup_bytes_and_digests(self) -> None:
        bundle = self._export()
        parsed, manifest = j2.load_bundle(bundle)
        files = j2.verify_bundle_files(bundle, parsed)
        target = self.root / "fresh-clone-inputs"
        result = j2.reimport_bundle(bundle, target)
        self.assertEqual(result["bundle_sha256"], sha(manifest))
        for relative, original in files.items():
            self.assertEqual((target / relative).read_bytes(), original, relative)
        self.assertEqual((target / "recording/index.tsv").read_bytes(), self.index)
        self.assertEqual((target / "adaptation/airspace.snapshot").read_bytes(), self.snapshot)
        self.assertEqual((target / "replay/settings.tsv").read_bytes(), self.settings)

    # REQ: OA-REG-020
    def test_recorded_profile_is_bound_through_export_reimport_and_replay_selector(self) -> None:
        self.assertEqual(j2.regional_run_profile(self.settings), ("DEFAULT_V1", None))
        old_settings_sha = sha(self.settings)
        lines = [
            "OA_REGIONAL_SETTINGS\t1",
            "centre\tZAU1",
            *[f"{name}\t{value}" for name, value in j2.RECORDED_PROFILE_SETTINGS.items()],
            "configuration\tSurveillanceConfig",
            "frame_gap_max_ns\t60000000000",
            "CA_h_m\t1300",
            "CA_v_ft\t300",
            "CA_lookahead_ns\t120000000000",
            "normalization\tpositive-address airborne positions",
            "model\tSUPPLIED_SNAPSHOT",
        ]
        self.settings = ("\n".join(lines) + "\n").encode()
        self._write(self.baseline / "settings.tsv", self.settings)
        self.run_manifest = self.run_manifest.replace(
            f"settings_sha256\t{old_settings_sha}".encode(),
            f"settings_sha256\t{sha(self.settings)}".encode(),
        )
        self._write(self.baseline / "manifest.tsv", self.run_manifest)
        self._write(self.baseline / "COMPLETE", (sha(self.run_manifest) + "\n").encode())

        self.assertEqual(
            j2.regional_run_profile(self.settings),
            (j2.RECORDED_PROFILE_ID, "recorded-peak-rendered"),
        )
        bundle = self._export("recorded-profile-bundle")
        parsed, _manifest = j2.load_bundle(bundle)
        files = j2.verify_bundle_files(bundle, parsed)
        self.assertEqual(files["replay/settings.tsv"], self.settings)
        target = self.root / "recorded-profile-reimport"
        j2.reimport_bundle(bundle, target)
        self.assertEqual((target / "replay/settings.tsv").read_bytes(), self.settings)

    def test_recorded_profile_rejects_unknown_duplicate_and_changed_capacity(self) -> None:
        with self.assertRaisesRegex(j2.DrillError, "unsupported run profile"):
            j2.regional_run_profile(b"OA_REGIONAL_SETTINGS\t1\nprofile\tOTHER_V1\n")
        with self.assertRaisesRegex(j2.DrillError, "duplicate field"):
            j2.regional_run_profile(
                b"OA_REGIONAL_SETTINGS\t1\nprofile\tRECORDED_PEAK_RENDERED_V1\n"
                b"profile\tRECORDED_PEAK_RENDERED_V1\n"
            )
        fields = [
            "OA_REGIONAL_SETTINGS\t1",
            "centre\tZAU1",
            *[f"{name}\t{value}" for name, value in j2.RECORDED_PROFILE_SETTINGS.items()],
        ]
        settings = "\n".join(fields) + "\n"
        for name in ("tracks", "flights"):
            changed = settings.replace(f"{name}\t4096", f"{name}\t32").encode()
            with self.assertRaisesRegex(j2.DrillError, "capacities do not match"):
                j2.regional_run_profile(changed)

    def test_empty_recording_or_zero_frame_baseline_is_refused(self) -> None:
        empty = b"OA-RS\t1\n"
        self._write(self.recording / "normalized.tsv", empty)
        self.index = self.index.replace(
            f"normalized_bytes\t{len(self.input)}".encode(), f"normalized_bytes\t{len(empty)}".encode()
        ).replace(sha(self.input).encode(), sha(empty).encode())
        self._write(self.recording / "manifest.tsv", self.index)
        self._write(self.recording / "COMPLETE", (sha(self.index) + "\n").encode())
        with self.assertRaisesRegex(j2.DrillError, "no event rows"):
            self._export()

        self._reset()
        self.run_manifest = self.run_manifest.replace(b"frame\t1\t1000000000", b"notframe\t1\t1000000000")
        self._write(self.baseline / "manifest.tsv", self.run_manifest)
        self._write(self.baseline / "COMPLETE", (sha(self.run_manifest) + "\n").encode())
        with self.assertRaisesRegex(j2.DrillError, "zero frames"):
            self._export("zero-frames")

    # REQ: OA-REG-020
    def test_sidecars_must_match_the_recording_import_manifest(self) -> None:
        self._write(self.recording / "provenance.tsv", self.provenance + b"tampered\n")
        with self.assertRaisesRegex(j2.DrillError, "provenance sidecar"):
            self._export()

    def test_bad_completion_adaptation_binding_tamper_and_public_output_refuse(self) -> None:
        self._write(self.recording / "COMPLETE", b"0" * 64 + b"\n")
        with self.assertRaisesRegex(j2.DrillError, "does not bind"):
            self._export("bad-import")

        self._reset()
        self._write(self.adaptation / "manifest.oag2",
                    self.adapt_manifest[:26] + bytes((self.adapt_manifest[26] ^ 1,))
                    + self.adapt_manifest[27:])
        with self.assertRaisesRegex(j2.DrillError, "does not bind"):
            self._export("bad-adaptation")

        self._reset()
        public = Path(self.temp.name) / "public-output"
        with self.assertRaisesRegex(j2.DrillError, "must stay under"):
            j2.export_bundle(self.recording, self.adaptation, self.baseline, "1" * 40, public)

        self._reset()
        bundle = self._export("tamper-bundle")
        (bundle / "recording/normalized.tsv").write_bytes(b"changed\n")
        with self.assertRaisesRegex(j2.DrillError, "digest mismatch"):
            parsed, _ = j2.load_bundle(bundle)
            j2.verify_bundle_files(bundle, parsed)

    def test_frame_manifest_uses_actual_sequence_receipt_filename_columns(self) -> None:
        _fields, frames = j2.parse_regional_manifest(self.run_manifest)
        j2.verify_frame_files(self.baseline, frames)
        bad = [frames[0].copy()]
        bad[0][3] = "../frame-0001.txt"
        with self.assertRaisesRegex(j2.DrillError, "frame filename"):
            j2.verify_frame_files(self.baseline, bad)

    def test_adaptation_manifest_v2_is_explicitly_refused(self) -> None:
        version_two = self.adapt_manifest[:8] + struct.pack("<H", 2) + self.adapt_manifest[10:]
        with self.assertRaisesRegex(j2.DrillError, "unsupported OAG2 adaptation manifest version 2"):
            j2.validate_adaptation_package(version_two, self.snapshot)

    def test_replay_path_guard_runs_before_any_subprocess(self) -> None:
        bundle = self._export("guard-order-bundle")
        repo = self.root / "checkout"
        repo.mkdir()
        binary = repo / "runner"
        binary.write_bytes(b"synthetic executable")
        binary.chmod(0o700)
        reimport = self.root / "reimport"
        output = self.root / "replay-output"

        def refuse_repo(path: Path, label: str) -> Path:
            if path == repo:
                raise j2.DrillError("synthetic repository path refusal")
            return path

        with mock.patch.object(j2, "guarded_existing_path", side_effect=refuse_repo):
            with mock.patch.object(j2.subprocess, "run") as run:
                with self.assertRaisesRegex(j2.DrillError, "synthetic repository path refusal"):
                    j2.run_replay(repo, binary, bundle, reimport, output)
                run.assert_not_called()

    def test_cold_drill_rejects_outputs_nested_in_source_before_git(self) -> None:
        bundle = self._export("cold-alias-bundle")
        source = self.root / "source"
        source.mkdir()
        clone = source / "clone"
        cargo_home = self.root / "cargo-home"
        reimport = self.root / "cold-reimport"
        output = self.root / "cold-output"
        with mock.patch.object(j2.subprocess, "run") as run:
            with self.assertRaisesRegex(j2.DrillError, "source checkout and fresh clone paths must be disjoint"):
                j2.cold_portability_drill(source, bundle, clone, cargo_home, reimport, output)
            run.assert_not_called()

    def test_cold_drill_rejects_outputs_nested_in_bundle_before_git(self) -> None:
        bundle = self._export("cold-bundle-alias")
        source = self.root / "source"
        source.mkdir()
        output = bundle / "result"
        with mock.patch.object(j2.subprocess, "run") as run:
            with self.assertRaisesRegex(j2.DrillError, "bundle and result output paths must be disjoint"):
                j2.cold_portability_drill(
                    source,
                    bundle,
                    self.root / "clone",
                    self.root / "cargo-home",
                    self.root / "reimport",
                    output,
                )
            run.assert_not_called()

    def test_measured_executable_digest_must_match_run_manifest(self) -> None:
        measured = "a" * 64
        j2.require_executable_digest_matches({"executable_sha256": measured}, measured)
        with self.assertRaisesRegex(j2.DrillError, "differs from the executed binary"):
            j2.require_executable_digest_matches({"executable_sha256": "b" * 64}, measured)

    def test_fresh_cargo_artifact_must_match_the_exact_binary_target(self) -> None:
        clone = self.root / "fresh-clone"
        manifest_dir = clone / "tools" / "oa-regional-session"
        source = manifest_dir / "src" / "main.rs"
        target = clone / "target"
        source.parent.mkdir(parents=True)
        (manifest_dir / "Cargo.toml").write_text("[package]\nname='oa-regional-session'\n", encoding="utf-8")
        source.write_text("// synthetic source path\n", encoding="utf-8")
        message = {
            "reason": "compiler-artifact",
            "package_id": f"path+{manifest_dir.resolve().as_uri()}#0.0.1",
            "manifest_path": str((manifest_dir / "Cargo.toml").resolve()),
            "target": {"name": "oa-regional-session", "kind": ["bin"], "src_path": str(source.resolve())},
            "fresh": False,
            "executable": str(target / "debug" / "oa-regional-session"),
            "filenames": [str(target / "debug" / "oa-regional-session")],
        }
        binary = j2.cold_cargo_executable((json.dumps(message) + "\n").encode(), clone, target)
        self.assertEqual(binary, target / "debug" / "oa-regional-session")
        wrong = dict(message)
        wrong["manifest_path"] = "/other/Cargo.toml"
        with self.assertRaisesRegex(j2.DrillError, "unexpected manifest"):
            j2.cold_cargo_executable((json.dumps(wrong) + "\n").encode(), clone, target)

    def test_hardlinked_cargo_output_copies_to_verified_single_link_executable(self) -> None:
        target = self.root / "target"
        binary = target / "debug" / "oa-regional-session"
        peer = target / "debug" / "deps" / "oa_regional_session-deadbeefcafefeed"
        binary.parent.mkdir(parents=True)
        peer.parent.mkdir()
        content = b"synthetic compiled executable bytes"
        binary.write_bytes(content)
        binary.chmod(0o700)
        peer.hardlink_to(binary)
        copied = target / "debug" / ".j2-replay" / "oa-regional-session"
        receipt = j2.copy_fresh_cargo_executable(binary, copied, binary, target)
        self.assertEqual(copied.read_bytes(), content)
        self.assertEqual(copied.stat().st_nlink, 1)
        self.assertEqual(receipt["source_hardlinks"], 2)
        self.assertEqual(receipt["sha256"], sha(content))

    def test_cargo_copy_refuses_wrong_path_symlink_and_unlisted_extra_link(self) -> None:
        target = self.root / "target"
        debug = target / "debug"
        debug.mkdir(parents=True)
        expected = debug / "oa-regional-session"
        real = debug / "real-executable"
        real.write_bytes(b"synthetic executable")
        real.chmod(0o700)
        expected.symlink_to(real)
        destination = debug / ".j2-replay" / "oa-regional-session"
        with self.assertRaisesRegex(j2.DrillError, "exact cold Cargo artifact"):
            j2.copy_fresh_cargo_executable(real, destination, expected, target)
        with self.assertRaisesRegex(j2.DrillError, "non-symlink"):
            j2.copy_fresh_cargo_executable(expected, destination, expected, target)
        expected.unlink()
        expected.hardlink_to(real)
        (debug / "extra-link").hardlink_to(real)
        (debug / "extra-link-2").hardlink_to(real)
        with self.assertRaisesRegex(j2.DrillError, "unexpected hardlink count"):
            j2.copy_fresh_cargo_executable(expected, destination, expected, target)

if __name__ == "__main__":
    unittest.main()
