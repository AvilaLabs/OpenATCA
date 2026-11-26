"""Focused synthetic tests for the F1a metadata-backed dependency graph guard."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import check_f1a_dependency_graph as f1a


class F1aGraphTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="oa-f1a-")
        self.root = Path(self.temp.name)
        self.external_dirs: list[Path] = []
        (self.root / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")

    def tearDown(self) -> None:
        for directory in self.external_dirs:
            shutil.rmtree(directory, ignore_errors=True)
        self.temp.cleanup()

    def pkg(self, name: str, cls: str = "shell") -> dict:
        directory = self.root / "crates" / name
        directory.mkdir(parents=True, exist_ok=True)
        manifest = directory / "Cargo.toml"
        manifest.write_text(
            f'[package]\nname = "{name}"\n[package.metadata.openatca]\nclass = "{cls}"\n',
            encoding="utf-8",
        )
        return {
            "name": name,
            "manifest_path": str(manifest),
            "metadata": {"openatca": {"class": cls}},
            "dependencies": [],
        }

    @staticmethod
    def edge(name: str, path: str | None, *, kind: str | None = None, target: str | None = None,
             source: str | None = None, registry: str | None = None, rename: str | None = None) -> dict:
        return {
            "name": name,
            "rename": rename,
            "kind": kind,
            "target": target,
            "path": path,
            "source": source,
            "registry": registry,
        }

    def metadata(self, *packages: dict) -> dict:
        return {"packages": list(packages)}

    # REQ: OA-TRC-266
    def test_outside_path_needs_exact_approval_reference(self) -> None:
        owner = self.pkg("owner")
        external = self.root.parent / f"outside-package-{self.root.name}"
        self.external_dirs.append(external)
        (external / "src").mkdir(parents=True, exist_ok=True)
        owner["dependencies"] = [self.edge("vendor", str(external))]
        errors = f1a.inspect_metadata(self.metadata(owner), self.root)
        self.assertTrue(any("outside path lacks adjacent F1-APPROVAL" in e for e in errors), errors)
        (Path(owner["manifest_path"])).write_text(
            '[package]\nname="owner"\n[dependencies]\nvendor = { path = "../../outside-package" } # F1-APPROVAL: F1-DEC-204\n',
            encoding="utf-8",
        )
        self.assertEqual([], f1a.inspect_metadata(self.metadata(owner), self.root))

    # REQ: OA-TRC-266
    def test_core_cannot_use_approved_external_path(self) -> None:
        core = self.pkg("core", "core")
        outside = self.root.parent / f"external-core-{self.root.name}"
        self.external_dirs.append(outside)
        (outside / "src").mkdir(parents=True, exist_ok=True)
        Path(core["manifest_path"]).write_text(
            '[package]\nname="core"\n[dependencies]\next = { path = "../../external-core" } # F1-APPROVAL: F1-DEC-205\n',
            encoding="utf-8",
        )
        core["dependencies"] = [self.edge("ext", str(outside))]
        self.assertTrue(any("core-to-outside dependency forbidden" in e for e in f1a.inspect_metadata(self.metadata(core), self.root)))

    # REQ: OA-TRC-266
    def test_external_registry_source_needs_approval(self) -> None:
        shell = self.pkg("shell")
        shell["dependencies"] = [self.edge("vendor", None, source="registry+https://example.invalid/index", registry="https://example.invalid")]
        self.assertTrue(any("external dependency lacks adjacent F1-APPROVAL" in e for e in f1a.inspect_metadata(self.metadata(shell), self.root)))
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[dependencies]\nvendor = "1" # F1-APPROVAL: F1-DEC-206\n',
            encoding="utf-8",
        )
        self.assertEqual([], f1a.inspect_metadata(self.metadata(shell), self.root))

    # REQ: OA-TRC-266
    def test_generic_comment_is_not_an_approval(self) -> None:
        shell = self.pkg("shell")
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[dependencies]\nvendor = "1" # F1-APPROVAL: approved\n',
            encoding="utf-8",
        )
        shell["dependencies"] = [self.edge("vendor", None, source="registry+https://example.invalid/index")]
        self.assertTrue(any("lacks adjacent F1-APPROVAL" in e for e in f1a.inspect_metadata(self.metadata(shell), self.root)))

    def test_root_note_does_not_approve_a_direct_package_dependency(self) -> None:
        shell = self.pkg("shell")
        outside = self.root.parent / f"root-note-outside-{self.root.name}"
        self.external_dirs.append(outside)
        outside.mkdir(exist_ok=True)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=[]\n[workspace.dependencies]\n'
            'vendor={path="../../outside"} # F1-APPROVAL: F1-DEC-207\n',
            encoding="utf-8",
        )
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[dependencies]\nvendor={path="../../outside"}\n',
            encoding="utf-8",
        )
        shell["dependencies"] = [self.edge("vendor", str(outside))]
        self.assertTrue(any("outside path lacks adjacent F1-APPROVAL" in e for e in f1a.inspect_metadata(self.metadata(shell), self.root)))

    def test_approval_on_wrong_target_does_not_cover_selected_target(self) -> None:
        shell = self.pkg("shell")
        outside = self.root.parent / f"wrong-target-outside-{self.root.name}"
        self.external_dirs.append(outside)
        outside.mkdir(exist_ok=True)
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[target.\'cfg(unix)\'.dependencies]\n'
            'vendor={path="../../outside"}\n[target.\'cfg(windows)\'.dependencies]\n'
            'vendor={path="../../outside"} # F1-APPROVAL: F1-DEC-208\n',
            encoding="utf-8",
        )
        shell["dependencies"] = [self.edge("vendor", str(outside), target="cfg(unix)")]
        self.assertTrue(any("outside path lacks adjacent F1-APPROVAL" in e for e in f1a.inspect_metadata(self.metadata(shell), self.root)))

    def test_approval_on_wrong_dependency_kind_does_not_apply(self) -> None:
        shell = self.pkg("shell")
        outside = self.root.parent / f"wrong-kind-outside-{self.root.name}"
        self.external_dirs.append(outside)
        outside.mkdir(exist_ok=True)
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[dependencies]\nvendor={path="../../outside"}\n'
            '[dev-dependencies]\nvendor={path="../../outside"} # F1-APPROVAL: F1-DEC-209\n',
            encoding="utf-8",
        )
        shell["dependencies"] = [self.edge("vendor", str(outside))]
        self.assertTrue(any("outside path lacks adjacent F1-APPROVAL" in e for e in f1a.inspect_metadata(self.metadata(shell), self.root)))

    def test_approval_must_match_cargo_dependency_alias(self) -> None:
        shell = self.pkg("shell")
        outside = self.root.parent / f"wrong-alias-outside-{self.root.name}"
        self.external_dirs.append(outside)
        outside.mkdir(exist_ok=True)
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[dependencies]\n'
            'actual_name={package="actual-name",path="../../outside"} # F1-APPROVAL: F1-DEC-211\n',
            encoding="utf-8",
        )
        shell["dependencies"] = [self.edge(
            "actual-name", str(outside), rename="other_alias",
        )]
        self.assertTrue(any("outside path lacks adjacent F1-APPROVAL" in e for e in f1a.inspect_metadata(self.metadata(shell), self.root)))

    def test_workspace_note_applies_only_through_exact_inherited_member_edge(self) -> None:
        shell = self.pkg("shell")
        outside = self.root.parent / f"workspace-inherited-outside-{self.root.name}"
        self.external_dirs.append(outside)
        outside.mkdir(exist_ok=True)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=[]\n[workspace.dependencies]\n'
            'vendor={path="../../outside"} # F1-APPROVAL: F1-DEC-210\n',
            encoding="utf-8",
        )
        Path(shell["manifest_path"]).write_text(
            '[package]\nname="shell"\n[dependencies]\nvendor={workspace=true}\n',
            encoding="utf-8",
        )
        shell["dependencies"] = [self.edge("vendor", str(outside))]
        self.assertEqual([], f1a.inspect_metadata(self.metadata(shell), self.root))

    # REQ: OA-TRC-266
    def test_unknown_internal_path_fails_even_when_inside_workspace(self) -> None:
        owner = self.pkg("owner")
        ghost = self.root / "crates" / "ghost"
        ghost.mkdir()
        owner["dependencies"] = [self.edge("ghost", str(ghost))]
        self.assertTrue(any("internal path does not resolve to a known workspace package/class" in e for e in f1a.inspect_metadata(self.metadata(owner), self.root)))

    # REQ: OA-TRC-266
    def test_dev_and_target_edges_participate_in_cycle_detection(self) -> None:
        one = self.pkg("one")
        two = self.pkg("two")
        one["dependencies"] = [self.edge("two", str(Path(two["manifest_path"]).parent), kind="dev", target="cfg(unix)")]
        two["dependencies"] = [self.edge("one", str(Path(one["manifest_path"]).parent), kind="build")]
        errors = f1a.inspect_metadata(self.metadata(one, two), self.root)
        self.assertTrue(any("dependency cycle" in e and "dev/cfg(unix)" in e for e in errors), errors)

    # REQ: OA-TRC-266
    def test_core_to_shell_edge_fails_for_all_edge_kinds(self) -> None:
        core = self.pkg("core", "core")
        shell = self.pkg("shell", "shell")
        core["dependencies"] = [self.edge("shell", str(Path(shell["manifest_path"]).parent), kind="build")]
        self.assertTrue(any("core-to-shell/tool edge forbidden" in e for e in f1a.inspect_metadata(self.metadata(core, shell), self.root)))

    # REQ: OA-TRC-266
    def test_current_workspace_metadata_check(self) -> None:
        repo = Path(__file__).resolve().parents[1]
        result = subprocess.run(
            ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
            cwd=repo, text=True, capture_output=True, check=False,
        )
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual([], f1a.inspect_metadata(json.loads(result.stdout), repo))


if __name__ == "__main__":
    unittest.main(verbosity=2)
