"""Source-to-registry inventory checks for Kani proof obligations."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import proof_source_inventory as inventory


class ProofSourceInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        subprocess.run(["git", "init", "--quiet"], cwd=self.root, check=True)
        self._write("Cargo.toml", '[workspace]\nmembers = ["crates/oa-track-core", "tools/oa-trace", "crates/sample"]\n')
        self._package("crates/oa-track-core", "oa-track-core")
        self._package("tools/oa-trace", "oa-trace")
        self._package("crates/sample", "sample")
        self._write("crates/sample/src/lib.rs", "mod proofs;\n")
        self._write("crates/sample/src/proofs.rs", "#[kani::proof]\nfn source_case() {}\n")
        self._registry([])

    def tearDown(self):
        self.temporary.cleanup()

    def _write(self, relative, content, *, track=True):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        if track:
            subprocess.run(["git", "add", "--", relative], cwd=self.root, check=True)
        return path

    def _package(self, relative, name):
        self._write(f"{relative}/Cargo.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\n')

    def _registry(self, proofs):
        self._write("tools/rust_proofs.json", json.dumps({"proofs": proofs}))

    def _proof(self):
        return {"package": "sample", "harness": "proofs::source_case"}

    def test_unregistered_source_harness_fails_source_inward(self):
        result = inventory.check_paths(self.root, self.root / "tools/rust_proofs.json")
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(result["unaccounted_source_harnesses"], [{
            "package": "sample", "harness": "proofs::source_case",
            "path": "crates/sample/src/proofs.rs", "line": 1,
        }])

    def test_registered_identity_and_named_exclusion_account_for_source(self):
        self._registry([self._proof()])
        result = inventory.check_paths(self.root, self.root / "tools/rust_proofs.json")
        self.assertEqual(result["status"], "PASS")
        exclusions = self._write("tools/exclusions.json", json.dumps({"exclusions": [
            {**self._proof(), "reason": "not selected for the bounded registry"},
        ]}))
        with self.assertRaisesRegex(inventory.InventoryError, "both registered and excluded"):
            inventory.check_paths(self.root, self.root / "tools/rust_proofs.json", exclusions)

    def test_exclusion_requires_exact_identity_and_nonempty_reason(self):
        exclusions = self._write("tools/exclusions.json", json.dumps({"exclusions": [self._proof()]}))
        with self.assertRaisesRegex(inventory.InventoryError, "requires a named reason"):
            inventory.check_paths(self.root, self.root / "tools/rust_proofs.json", exclusions)
        exclusions.write_text(json.dumps({"exclusions": [
            {**self._proof(), "reason": "source harness is intentionally not registered"},
        ]}), encoding="utf-8")
        self.assertEqual(inventory.check_paths(
            self.root, self.root / "tools/rust_proofs.json", exclusions
        )["status"], "PASS")

    def test_path_attribute_maps_harness_to_declaring_module(self):
        self._write("crates/sample/src/lib.rs", "mod wire;\n")
        self._write("crates/sample/src/proofs.rs", "")
        self._write("crates/sample/src/wire.rs", '#[path = "wire_proofs.rs"]\nmod proofs;\n')
        self._write("crates/sample/src/wire_proofs.rs", "#[kani::proof]\nfn wire_case() {}\n")
        source = inventory.discover_source_proofs(self.root)
        self.assertEqual([(item.package, item.harness) for item in source],
                         [("sample", "wire::proofs::wire_case")])

    def test_conventional_nested_modules_map_to_their_file_path(self):
        self._write("crates/sample/src/lib.rs", "mod navigation;\n")
        self._write("crates/sample/src/proofs.rs", "")
        self._write("crates/sample/src/navigation.rs", "pub mod checks;\n")
        self._write("crates/sample/src/navigation/checks.rs",
                    "#[kani::proof]\nfn nested_case() {}\n")
        source = inventory.discover_source_proofs(self.root)
        self.assertEqual([(item.package, item.harness) for item in source],
                         [("sample", "navigation::checks::nested_case")])

    def test_inline_and_cfg_disabled_module_proofs_are_still_inventory_entries(self):
        self._write(
            "crates/sample/src/lib.rs",
            "#[cfg(any())]\nmod disabled { mod proofs { #[kani::proof] fn hidden_case() {} } }\n",
        )
        self._write("crates/sample/src/proofs.rs", "")
        source = inventory.discover_source_proofs(self.root)
        self.assertEqual([(item.package, item.harness) for item in source],
                         [("sample", "disabled::proofs::hidden_case")])

    def test_ambiguous_same_name_module_paths_refuse_inventory(self):
        self._write(
            "crates/sample/src/lib.rs",
            "mod duplicate { #[kani::proof] fn same_case() {} }\n"
            "mod duplicate { #[kani::proof] fn same_case() {} }\n",
        )
        with self.assertRaisesRegex(inventory.InventoryError, "duplicate source package/harness"):
            inventory.discover_source_proofs(self.root)

    def test_path_attribute_cannot_escape_package_root(self):
        self._write("crates/sample/src/lib.rs", '#[path = "../../../outside.rs"]\nmod proofs;\n')
        with self.assertRaisesRegex(inventory.InventoryError, "escapes its package"):
            inventory.discover_source_proofs(self.root)

    def test_stale_registry_identity_fails_closed(self):
        self._registry([{"package": "sample", "harness": "proofs::renamed_case"}])
        result = inventory.check_paths(self.root, self.root / "tools/rust_proofs.json")
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(result["stale_registry_or_exclusion_identities"], [{
            "package": "sample", "harness": "proofs::renamed_case",
        }])

    def test_unrepresentable_proof_attribute_refuses_inventory(self):
        self._write("crates/sample/src/proofs.rs", "#[kani::proof]\nconst NOT_A_FUNCTION: u8 = 1;\n")
        with self.assertRaisesRegex(inventory.InventoryError, "not attached to a function"):
            inventory.discover_source_proofs(self.root)

    def test_proof_outside_library_source_is_not_silently_skipped(self):
        self._write("crates/sample/tests/external.rs", "#[kani::proof]\nfn outside_lib() {}\n")
        with self.assertRaisesRegex(inventory.InventoryError, "outside package src"):
            inventory.discover_source_proofs(self.root)

    def test_tracked_proof_outside_supported_package_sources_fails_closed(self):
        self._write("validation/inventory_probe.rs", "#[kani::proof]\nfn unowned_case() {}\n")
        with self.assertRaisesRegex(inventory.InventoryError, "outside a supported package src"):
            inventory.discover_source_proofs(self.root)

    def test_ignored_untracked_target_rust_is_not_selected_as_source(self):
        self._write(".gitignore", "/target/\n")
        cache_path = "target/generated/probe.rs"
        self._write(cache_path, "#[kani::proof]\nfn cache_case() {}\n", track=False)
        ignored = subprocess.run(
            ["git", "check-ignore", "--quiet", "--", cache_path], cwd=self.root,
            check=False,
        )
        self.assertEqual(ignored.returncode, 0)
        tracked = subprocess.run(
            ["git", "ls-files", "--error-unmatch", "--", cache_path], cwd=self.root,
            check=False, capture_output=True,
        )
        self.assertNotEqual(tracked.returncode, 0)
        result = inventory.discover_source_proofs(self.root)
        self.assertEqual([(item.package, item.harness) for item in result],
                         [("sample", "proofs::source_case")])

    def test_workspace_source_inventory_is_fully_accounted(self):
        workspace = Path(__file__).resolve().parents[1]
        result = inventory.check_paths(workspace, workspace / "tools/rust_proofs.json")
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["source_harness_count"], result["registered_source_harness_count"])
        self.assertEqual(result["unaccounted_source_harnesses"], [])
        self.assertEqual(result["stale_registry_or_exclusion_identities"], [])

    def test_wrong_root_missing_root_and_filesystem_root_refuse(self):
        with self.assertRaises(inventory.InventoryError):
            inventory.check_paths(self.root / "missing", self.root / "tools/rust_proofs.json")
        wrong = Path(tempfile.mkdtemp())
        try:
            (wrong / "Cargo.toml").write_text('[workspace]\nmembers = []\n', encoding="utf-8")
            with self.assertRaises(inventory.InventoryError):
                inventory.check_paths(wrong, wrong / "tools/rust_proofs.json")
        finally:
            import shutil
            shutil.rmtree(wrong)
        with self.assertRaises(inventory.InventoryError):
            inventory.check_paths(Path("/"), Path("/tools/rust_proofs.json"))


if __name__ == "__main__":
    unittest.main()
