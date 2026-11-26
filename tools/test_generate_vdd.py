"""Synthetic coverage for each VDD section and the clean-source gate."""
from __future__ import annotations

import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import generate_vdd


class VddFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="openatca-vdd-test-")
        self.base = Path(self.temp.name)
        self.repo = self.base / "repo"
        self.repo.mkdir()
        self._write("Cargo.toml", '[workspace.package]\nversion = "1.2.3"\n')
        self._write("Cargo.lock", "version = 4\n")
        self._write("rust-toolchain.toml", '[toolchain]\nchannel = "1.89.0"\n')
        self._write("python/pyproject.toml", '[project]\nversion = "0.4.2"\n')
        self._write("tools/oa-adapt/src/manifest.rs", "const LEGACY_MANIFEST_VERSION: u16 = 1;\nconst ISSUED_MANIFEST_VERSION: u16 = 2;\nconst CA_PRESET_MANIFEST_VERSION: u16 = 3;\n")
        self._write("crates/oa-airspace/src/core/snapshot.rs", "pub const FORMAT_VERSION: u32 = 3;\n")
        self._write("REQUIREMENTS.md", "- **OA-GEN-001** smoke\n")
        self._write("docs/safety/hazard-log.json", '{"schema":"test"}\n')
        self._write("docs/safety/generated/pha.csv", "id\n")
        self._write("docs/safety/generated/osa.csv", "id\n")
        self._write("docs/safety/generated/ssar.csv", "id\n")
        self._write("docs/STATUS.md", "| check | state |\n|---|---|\n| tests | partial |\n")
        self._write("docs/CAPABILITIES.md", "| area | limit |\n|---|---|\n| route | partial coverage |\n")
        self._write("docs/UNKNOWNS.md", "| id | unknown |\n|---|---|\n| UK-1 | unresolved |\n")
        self.trace = self.base / "trace.txt"
        self.trace.write_text(
            "Requirements with tests (1):\n  OA-GEN-001\n\n"
            "Requirements WITHOUT tests (0):\n\n"
            "Traceability (every requirement linked, ids unique; no test or proof run): PASS\n",
            encoding="utf-8",
        )
        self.build = self.base / "release-bins"
        self.build.mkdir()
        binary = self.build / "oa-test"
        binary.write_bytes(b"binary fixture")
        binary.chmod(0o755)
        self.provenance = self.base / "toolchain-provenance.json"
        self.provenance.write_text(
            '{"toolchain":{"rustc_fields":{"release":"1.89.0"},'
            '"cargo_fields":{"release":"1.89.0"}},'
            '"host":{"kernel":{"machine":"x86_64"},'
            '"os_release":{"pretty_name":"Test Linux"}}}\n', encoding="utf-8"
        )
        (self.base / "sbom.spdx.json").write_text('{"spdxVersion":"SPDX-2.3"}\n', encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        subprocess.run(["git", "-C", str(self.repo), "config", "user.email", "test@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "config", "user.name", "VDD test"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "add", "."], check=True)
        subprocess.run(["git", "-C", str(self.repo), "commit", "-qm", "initial baseline"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "tag", "v1.2.3"], check=True)
        self._write("change.txt", "updated\n")
        subprocess.run(["git", "-C", str(self.repo), "add", "."], check=True)
        subprocess.run(["git", "-C", str(self.repo), "commit", "-qm",
                        "OA-GEN-001: bounded VDD test relative/private/note"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "tag", "-a", "-m", "fixture release", "v1.2.4"], check=True)

    def tearDown(self):
        self.temp.cleanup()

    def _write(self, rel, text):
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def render(self):
        return generate_vdd.generate(
            self.repo, self.base / "unused.md", mode="release", tag="v1.2.4",
            release_date="2026-11-26", previous_tag="v1.2.3",
            trace_file=self.trace, build_dir=self.build, provenance_file=self.provenance,
        ).decode("utf-8")

    def test_section_1_release_identity(self):
        text = self.render()
        self.assertIn("## 1. Release identity", text)
        self.assertIn("Version: `1.2.4`", text)
        self.assertIn("Release tag: `v1.2.4`", text)
        self.assertIn("Date: `2026-11-26`", text)

    def test_annotated_release_tag_uses_target_commit_date(self):
        tagged = generate_vdd._git_text(self.repo, "rev-parse", "v1.2.4^{commit}", cap=256)
        expected = generate_vdd._git_text(self.repo, "show", "-s", "--format=%cs", tagged, cap=256)
        _, date, _, _ = generate_vdd._tag_info(self.repo, "release", "v1.2.4", None)
        self.assertEqual(date, expected)

    def test_section_2_item_versions(self):
        text = self.render()
        self.assertIn("## 2. Configuration items and versions", text)
        self.assertIn("`1.2.3`", text)
        self.assertIn("`0.4.2`", text)
        self.assertIn("`1, 2, 3`", text)

    def test_section_3_commit_change_requirements(self):
        text = self.render()
        self.assertIn("## 3. Changes since the prior baseline", text)
        self.assertIn("**OA-GEN-001**", text)
        self.assertIn("bounded VDD test", text)
        self.assertNotIn("private/note", text)

    def test_section_4_trace_summary_and_digest(self):
        text = self.render()
        self.assertIn("## 4. Requirements trace", text)
        self.assertIn("Traceability (every requirement linked", text)
        self.assertIn(hashlib.sha256(self.trace.read_bytes()).hexdigest(), text)

    def test_uncovered_trace_rows_are_named_as_open_requirements(self):
        self.trace.write_text(
            "Requirements with tests (0):\n\nRequirements WITHOUT tests (1):\n  OA-GEN-001\n\n"
            "Traceability (every requirement linked, ids unique; no test or proof run): FAIL\n",
            encoding="utf-8",
        )
        text = self.render()
        self.assertIn("Uncovered requirement IDs: OA-GEN-001", text)
        self.assertIn("Uncovered requirements: OA-GEN-001", text)

    def test_section_5_hazard_commit_and_generated_table_digests(self):
        text = self.render()
        self.assertIn("## 5. Hazard record", text)
        for name in ("pha.csv", "osa.csv", "ssar.csv"):
            self.assertIn(name, text)

    def test_section_6_only_public_limit_registers_are_used(self):
        text = self.render()
        self.assertIn("## 6. Known problems and limits", text)
        self.assertIn("UK-1", text)
        self.assertIn("partial, unmet, open or unknown state", text)
        self.assertNotIn("private acceptance", text)

    def test_section_7_build_environment_and_output_digest(self):
        text = self.render()
        self.assertIn("## 7. Build environment and output digests", text)
        self.assertIn("oa-test", text)
        self.assertIn(hashlib.sha256(b"binary fixture").hexdigest(), text)
        self.assertIn("Test Linux", text)
        self.assertIn("Cargo: `1.89.0`", text)
        self.assertIn("Python: `", text)
        self.assertIn(hashlib.sha256(self.provenance.read_bytes()).hexdigest(), text)
        self.assertIn(hashlib.sha256((self.base / "sbom.spdx.json").read_bytes()).hexdigest(), text)

    def test_section_8_adaptation_compatibility(self):
        text = self.render()
        self.assertIn("## 8. Installation and adaptation compatibility", text)
        self.assertIn("adaptation", text)

    def test_dirty_source_refuses_before_render(self):
        (self.repo / "untracked.txt").write_text("dirty\n", encoding="utf-8")
        with self.assertRaisesRegex(generate_vdd.VddError, "dirty source tree"):
            self.render()

    def test_named_trace_and_history_work_caps_refuse(self):
        with patch.object(generate_vdd, "MAX_TRACE_BYTES", 8):
            with self.assertRaisesRegex(generate_vdd.VddError, "byte limit"):
                generate_vdd._trace_summary(self.trace)
        with patch.object(generate_vdd, "MAX_COMMITS", 0):
            with self.assertRaisesRegex(generate_vdd.VddError, "commit limit"):
                generate_vdd._changes(self.repo, "v1.2.3", "v1.2.4")
        with patch.object(generate_vdd, "MAX_RELEASE_TAGS", 0):
            with self.assertRaisesRegex(generate_vdd.VddError, "tag count"):
                generate_vdd._previous_tag(self.repo, "candidate", None)

    def test_explicit_previous_tag_must_be_older_and_distinct(self):
        with self.assertRaisesRegex(generate_vdd.VddError, "differ from the current"):
            generate_vdd._previous_tag(self.repo, "v1.2.4", "v1.2.4")
        with self.assertRaisesRegex(generate_vdd.VddError, "lower semantic version"):
            generate_vdd._previous_tag(self.repo, "v1.2.4", "v2.0.0")
        with self.assertRaises(generate_vdd.VddError):
            generate_vdd._previous_tag(self.repo, "v1.2.4", "v1.0.0")

    def test_public_status_and_build_caps_refuse(self):
        with patch.object(generate_vdd, "MAX_PUBLIC_LIMIT_ROWS", 0):
            with self.assertRaisesRegex(generate_vdd.VddError, "rows exceed"):
                generate_vdd._public_limits(self.repo)
        with patch.object(generate_vdd, "MAX_TEXT_SOURCE_BYTES", 4):
            with self.assertRaisesRegex(generate_vdd.VddError, "byte limit"):
                generate_vdd._public_limits(self.repo)
        with patch.object(generate_vdd, "MAX_BUILD_FILES", 0):
            with self.assertRaisesRegex(generate_vdd.VddError, "entries exceed"):
                generate_vdd._build_outputs(self.build)
        with patch.object(generate_vdd, "MAX_ONE_BUILD_FILE_BYTES", 4):
            with self.assertRaisesRegex(generate_vdd.VddError, "per-file limit"):
                generate_vdd._build_outputs(self.build)
        with patch.object(generate_vdd, "MAX_TOTAL_BUILD_BYTES", 4):
            with self.assertRaisesRegex(generate_vdd.VddError, "aggregate byte limit"):
                generate_vdd._build_outputs(self.build)

    def test_output_growth_is_refused_before_append(self):
        report = generate_vdd.BoundedText(4)
        report.append("1234")
        with self.assertRaisesRegex(generate_vdd.VddError, "rendered text exceeds"):
            report.append("5")

    def test_command_output_cap_refuses_before_accumulating_more(self):
        with self.assertRaisesRegex(generate_vdd.VddError, "command output exceeds"):
            generate_vdd._run(self.repo, [sys.executable, "-c", "print('0123456789')"], cap=4)


if __name__ == "__main__":
    unittest.main()
