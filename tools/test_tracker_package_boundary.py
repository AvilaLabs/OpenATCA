"""Architecture boundary checks for the reusable tracker/tools split (OA-TRK-413)."""

from __future__ import annotations

import tomllib
import unittest
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
TRACK_MANIFEST = ROOT / "crates/oa-track/Cargo.toml"
TOOLS_MANIFEST = ROOT / "crates/oa-track-tools/Cargo.toml"
PRIVATE_PACKAGES = {"oa-bench", "oa-sensorsim", "oa-sealed"}


def _dependency_names(table: Any) -> set[str]:
    """Return logical and renamed Cargo package names from one dependency table."""
    if not isinstance(table, dict):
        return set()
    names: set[str] = set()
    for key, spec in table.items():
        names.add(key.replace("_", "-"))
        if isinstance(spec, dict):
            package = spec.get("package")
            if isinstance(package, str):
                names.add(package.replace("_", "-"))
    return names


def _runtime_dependency_names(manifest: dict[str, Any]) -> set[str]:
    """Read runtime and build dependencies, including target-specific optional aliases."""
    names: set[str] = set()
    for section in ("dependencies", "build-dependencies"):
        names.update(_dependency_names(manifest.get(section)))
    targets = manifest.get("target", {})
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for section in ("dependencies", "build-dependencies"):
                names.update(_dependency_names(target.get(section)))
    return names


def _manifest(path: Path) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


class TrackerPackageBoundary(unittest.TestCase):
    # REQ: OA-TRK-413
    def test_reusable_tracker_excludes_private_tool_packages_from_all_runtime_sections(self) -> None:
        manifest = _manifest(TRACK_MANIFEST)
        forbidden = _runtime_dependency_names(manifest) & PRIVATE_PACKAGES
        self.assertEqual(forbidden, set(), f"private package leaked into runtime graph: {forbidden}")

        # Test-only simulator fixtures are permitted and do not enter normal consumers' graphs.
        dev = manifest.get("dev-dependencies", {})
        self.assertIn("oa-sensorsim", _dependency_names(dev))

    # REQ: OA-TRK-413
    def test_runtime_scanner_catches_alias_optional_and_target_dependencies(self) -> None:
        fixtures = {
            "renamed": """
[dependencies.sim]
package = "oa-sensorsim"
path = "../oa-sensorsim"
optional = true
""",
            "optional": """
[dependencies]
oa-bench = { path = "../oa-bench", optional = true }
""",
            "target-conditional": """
[target.'cfg(target_os = \"linux\")'.dependencies]
bench = { package = "oa-bench", path = "../oa-bench" }
""",
            "target-build": """
[target.'cfg(unix)'.build-dependencies]
sim = { package = "oa-sensorsim", path = "../oa-sensorsim" }
""",
        }
        for name, text in fixtures.items():
            with self.subTest(name=name):
                names = _runtime_dependency_names(tomllib.loads(text))
                self.assertTrue(names & PRIVATE_PACKAGES, f"did not detect {name}: {names}")

    # REQ: OA-TRK-413
    def test_dev_only_private_dependency_is_allowed(self) -> None:
        manifest = tomllib.loads(
            """
[dependencies]
oa-geo = { path = "../oa-geo" }

[dev-dependencies]
oa-sensorsim = { path = "../oa-sensorsim" }

[target.'cfg(test)'.dev-dependencies]
oa-bench = { path = "../oa-bench" }
"""
        )
        self.assertFalse(_runtime_dependency_names(manifest) & PRIVATE_PACKAGES)

    # REQ: OA-TRK-413
    def test_tracker_has_no_runtime_cli_targets_and_tools_preserve_binary_names_and_paths(self) -> None:
        tracker = _manifest(TRACK_MANIFEST)
        self.assertEqual(tracker.get("bin", []), [])
        self.assertFalse((ROOT / "crates/oa-track/src/main.rs").exists())
        self.assertFalse((ROOT / "crates/oa-track/src/fusion_main.rs").exists())

        tools = _manifest(TOOLS_MANIFEST)
        self.assertTrue(PRIVATE_PACKAGES <= _runtime_dependency_names(tools))
        self.assertIn("oa-track", _runtime_dependency_names(tools))
        bins = {item.get("name"): item.get("path") for item in tools.get("bin", [])}
        self.assertEqual(
            bins,
            {
                "oa-track-run": "src/main.rs",
                "oa-track-fusion": "src/fusion_main.rs",
            },
        )
        for relative in bins.values():
            self.assertTrue((TOOLS_MANIFEST.parent / relative).is_file(), relative)


if __name__ == "__main__":
    unittest.main()
