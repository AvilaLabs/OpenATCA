"""Stdlib-only tests for the private CORE source staging tool."""
from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


TOOL = Path(__file__).with_name("oa-core-release-stage.py")
SPEC = importlib.util.spec_from_file_location("oa_core_release_stage", TOOL)
assert SPEC is not None and SPEC.loader is not None
stage_tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(stage_tool)

ROOT_CARGO = '''[workspace]
resolver = "2"

[workspace.package]
version = "0.0.1"
edition = "2021"
license = "Apache-2.0"
rust-version = "1.89"
authors = ["Synthetic Test"]

[workspace.lints.rust]
unsafe_code = "forbid"
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(kani)", "cfg(kani_full_capacity)", "cfg(kani_late_replay_covers)", "cfg(creusot)"] }

[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"
'''

PACKAGE_CARGO = '''[package]
name = "oa-mini"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
readme = "README.md"

[lints]
workspace = true

[dependencies]
oa-base = { path = "../oa-base" }

[package.metadata.openatca]
class = "core"
'''

BASE_CARGO = '''[package]
name = "oa-base"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
readme = "README.md"

[lints]
workspace = true

[package.metadata.openatca]
class = "core"

[lib]
name = "base_api"
'''


class ReleaseStageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="oa-stage-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self._write("Cargo.toml", ROOT_CARGO)
        self._write("LICENSE", "Synthetic license\n")
        self._write("docs/core-api/test.md", "# Synthetic API\n")
        self._write("crates/oa-base/Cargo.toml", BASE_CARGO)
        self._write("crates/oa-base/README.md", "# Base\n")
        self._write("crates/oa-base/src/lib.rs", b"pub fn base() -> u32 { 7 }\n")
        self._write("crates/oa-mini/Cargo.toml", PACKAGE_CARGO)
        self._write("crates/oa-mini/README.md", "# Mini\n")
        self._write("crates/oa-mini/src/lib.rs", b"pub fn answer() -> u32 { 42 }\n")
        self._write("crates/oa-mini/tests/acceptance.rs", b"include_str!(\"fixtures/vector.txt\");\n")
        self._write("crates/oa-mini/tests/fixtures/vector.txt", b"vector bytes\n")
        self.api_patch = patch.dict(stage_tool.API_DOCS, {"oa-mini": "docs/core-api/test.md", "oa-base": "docs/core-api/test.md"})
        self.assets_patch = patch.dict(stage_tool.TEST_ASSETS, {"oa-mini": {"tests/fixtures/vector.txt"}})
        self.api_patch.start()
        self.assets_patch.start()
        self.candidates_patch = patch.object(stage_tool, "ROOT_CANDIDATES", ("oa-mini",))
        self.candidates_patch.start()
        self.addCleanup(self.api_patch.stop)
        self.addCleanup(self.assets_patch.stop)
        self.addCleanup(self.candidates_patch.stop)
        self._git("init", "-q")
        self._git("add", "-A")
        self._git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture")
        self.commit = self._git("rev-parse", "HEAD").strip()
        self.out_parent = self.root / "outputs"
        self.out_parent.mkdir()

    def _write(self, relative: str, data: str | bytes) -> None:
        path = self.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data.encode() if isinstance(data, str) else data)

    def _git(self, *args: str) -> str:
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True,
                              capture_output=True, text=True).stdout

    def _stage(self, name: str):
        return stage_tool.stage(self.repo, self.commit, self.out_parent / name, ("oa-mini",))

    def _commit_file_change(self) -> None:
        self._git("add", "-A")
        self._git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture-change")
        self.commit = self._git("rev-parse", "HEAD").strip()

    def test_success_copies_exact_inputs_and_transforms_public_manifest(self) -> None:
        manifest = self._stage("first")
        first = self.out_parent / "first"
        self.assertEqual((first / "crates/openatca-mini/src/lib.rs").read_bytes(),
                         self._git("show", f"{self.commit}:crates/oa-mini/src/lib.rs").encode())
        self.assertEqual((first / "crates/openatca-mini/tests/fixtures/vector.txt").read_bytes(), b"vector bytes\n")
        self.assertEqual((first / "crates/openatca-base/src/lib.rs").read_bytes(), b"pub fn base() -> u32 { 7 }\n")
        staged_manifest = (first / "crates/openatca-mini/Cargo.toml").read_text()
        self.assertIn('name = "openatca-mini"', staged_manifest)
        self.assertIn('version = "0.1.0"', staged_manifest)
        self.assertIn('edition = "2021"', staged_manifest)
        self.assertIn('license = "Apache-2.0"', staged_manifest)
        self.assertIn('rust-version = "1.89"', staged_manifest)
        self.assertIn('authors = ["Synthetic Test"]', staged_manifest)
        self.assertIn('oa-base = { path = "../openatca-base", package = "openatca-base", version = "0.1.0" }', staged_manifest)
        self.assertIn('"tests/fixtures/*"', staged_manifest)
        self.assertIn('[lib]\nname = "oa_mini"', staged_manifest)
        self.assertIn('[lib]\nname = "base_api"', (first / "crates/openatca-base/Cargo.toml").read_text())
        self.assertEqual(manifest["package_order"], ["openatca-base", "openatca-mini"])
        self.assertEqual(manifest["package_import_names"], {
            "openatca-base": "base_api",
            "openatca-mini": "oa_mini",
        })
        api_doc = (first / "crates/openatca-mini/API.md").read_text()
        self.assertIn("Cargo package: `openatca-mini`", api_doc)
        self.assertIn("Rust crate import: `oa_mini`", api_doc)
        self.assertEqual(manifest["source_files_sha256"]["crates/oa-mini/src/lib.rs"],
                         hashlib.sha256((self.repo / "crates/oa-mini/src/lib.rs").read_bytes()).hexdigest())
        release_manifest = json.loads((first / "release-manifest.json").read_text())
        self.assertEqual(release_manifest["staged_files_sha256"]["crates/openatca-mini/src/lib.rs"],
                         hashlib.sha256((first / "crates/openatca-mini/src/lib.rs").read_bytes()).hexdigest())

    def test_repeat_to_distinct_output_is_deterministic(self) -> None:
        self._stage("first")
        self._stage("second")
        first = self.out_parent / "first"
        second = self.out_parent / "second"
        first_files = sorted(p.relative_to(first) for p in first.rglob("*") if p.is_file())
        second_files = sorted(p.relative_to(second) for p in second.rglob("*") if p.is_file())
        self.assertEqual(first_files, second_files)
        self.assertEqual({p: (first / p).read_bytes() for p in first_files},
                         {p: (second / p).read_bytes() for p in second_files})

    def test_private_dependency_is_refused_before_output(self) -> None:
        private_manifest = BASE_CARGO.replace('name = "oa-base"', 'name = "oa-sealed"')
        private_manifest = private_manifest.replace('class = "core"', 'class = "shell"')
        self._write("crates/oa-base/Cargo.toml", private_manifest)
        self._write("crates/oa-mini/Cargo.toml", PACKAGE_CARGO.replace(
            'oa-base = { path = "../oa-base" }', 'oa-sealed = { path = "../oa-base" }'))
        self._commit_file_change()
        output = self.out_parent / "private"
        with self.assertRaisesRegex(stage_tool.StageError, "prohibited private"):
            stage_tool.stage(self.repo, self.commit, output, ("oa-mini",))
        self.assertFalse(output.exists())

    def test_missing_compile_time_fixture_is_refused(self) -> None:
        self._write("crates/oa-mini/tests/acceptance.rs", b'include_str!("fixtures/missing.txt");\n')
        self._commit_file_change()
        with self.assertRaisesRegex(stage_tool.StageError, "references unstaged file"):
            stage_tool.stage(self.repo, self.commit, self.out_parent / "missing", ("oa-mini",))

    def test_external_compile_time_fixture_is_refused(self) -> None:
        self._write("crates/oa-mini/src/lib.rs", b'include_bytes!("../../../../outside.bin");\n')
        self._commit_file_change()
        with self.assertRaisesRegex(stage_tool.StageError, "compile-time include escapes package"):
            stage_tool.stage(self.repo, self.commit, self.out_parent / "external", ("oa-mini",))

    def test_tracked_symlink_input_is_refused(self) -> None:
        link = self.repo / "crates/oa-mini/src/link.rs"
        link.symlink_to("lib.rs")
        self._git("add", "-A")
        self._git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "symlink")
        self.commit = self._git("rev-parse", "HEAD").strip()
        with self.assertRaisesRegex(stage_tool.StageError, "non-regular package input"):
            stage_tool.stage(self.repo, self.commit, self.out_parent / "symlink", ("oa-mini",))

    def test_existing_output_and_symlink_parent_are_refused(self) -> None:
        existing = self.out_parent / "existing"
        existing.mkdir()
        (existing / "sentinel").write_text("keep")
        with self.assertRaisesRegex(stage_tool.StageError, "must be a new path"):
            stage_tool.stage(self.repo, self.commit, existing, ("oa-mini",))
        self.assertEqual((existing / "sentinel").read_text(), "keep")
        alias = self.root / "linked-output"
        alias.symlink_to(self.out_parent, target_is_directory=True)
        with self.assertRaisesRegex(stage_tool.StageError, "parent path must not traverse symlink"):
            stage_tool.stage(self.repo, self.commit, alias / "child", ("oa-mini",))


if __name__ == "__main__":
    unittest.main()
