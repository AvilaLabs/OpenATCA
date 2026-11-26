from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import supply_chain_evidence as evidence


class SpdxInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "one").mkdir()
        (self.root / "two").mkdir()
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["one", "two"]\n'
            '[workspace.package]\nversion = "1.2.3"\nlicense = "Apache-2.0"\n',
            encoding="utf-8",
        )
        (self.root / "one/Cargo.toml").write_text(
            '[package]\nname = "one"\nversion.workspace = true\nlicense.workspace = true\n',
            encoding="utf-8",
        )
        (self.root / "two/Cargo.toml").write_text(
            '[package]\nname = "two"\nversion.workspace = true\nlicense.workspace = true\n',
            encoding="utf-8",
        )
        self.lock = (
            'version = 4\n\n[[package]]\nname = "one"\nversion = "1.2.3"\n'
            'dependencies = ["two"]\n\n[[package]]\nname = "two"\nversion = "1.2.3"\n'
        )
        (self.root / "Cargo.lock").write_text(self.lock, encoding="utf-8")

    def tearDown(self):
        self.temp.cleanup()

    def test_all_workspace_crates_and_lock_edges_are_in_spdx(self):
        document, third_party = evidence.make_spdx(self.root, "2026-11-25T00:00:00Z")
        self.assertEqual(document["spdxVersion"], "SPDX-2.3")
        self.assertEqual(document["dataLicense"], "CC0-1.0")
        self.assertEqual(third_party, 0)
        packages = {package["name"]: package for package in document["packages"]}
        self.assertEqual(set(packages), {"one", "two"})
        self.assertEqual(packages["one"]["licenseDeclared"], "Apache-2.0")
        self.assertIn({"spdxElementId": packages["one"]["SPDXID"],
                       "relationshipType": "DEPENDS_ON",
                       "relatedSpdxElement": packages["two"]["SPDXID"]},
                      document["relationships"])
        self.assertEqual(sum(row["relationshipType"] == "DESCRIBES"
                             for row in document["relationships"]), 2)
        self.assertIn("source-backed third-party packages=0",
                      document["creationInfo"]["comment"])

    def test_external_lock_source_is_counted_not_assumed_away(self):
        external = (
            'version = 4\n\n[[package]]\nname = "one"\nversion = "1.2.3"\n'
            'dependencies = ["serde 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"]\n\n'
            '[[package]]\nname = "two"\nversion = "1.2.3"\n\n'
            '[[package]]\nname = "serde"\nversion = "1.0.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "' + "a" * 64 + '"\n'
        )
        (self.root / "Cargo.lock").write_text(external, encoding="utf-8")
        document, count = evidence.make_spdx(self.root, "2026-11-25T00:00:00Z")
        self.assertEqual(count, 1)
        package = next(row for row in document["packages"] if row["name"] == "serde")
        self.assertIn("third-party", package["packageComment"])
        self.assertEqual(package["checksums"][0]["checksumValue"], "a" * 64)

    def test_missing_workspace_crate_and_unresolved_edge_refuse(self):
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname="one"\nversion="1.2.3"\n', encoding="utf-8")
        with self.assertRaises(evidence.EvidenceError):
            evidence.make_spdx(self.root)
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname="one"\nversion="1.2.3"\n'
            'dependencies=["missing"]\n[[package]]\nname="two"\nversion="1.2.3"\n',
            encoding="utf-8")
        with self.assertRaises(evidence.EvidenceError):
            evidence.make_spdx(self.root)

    def test_workspace_glob_and_manifest_symlink_refuse(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["one/*"]\n', encoding="utf-8")
        with self.assertRaises(evidence.EvidenceError):
            evidence.make_spdx(self.root)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["one", "two"]\n', encoding="utf-8")
        (self.root / "two/Cargo.toml").unlink()
        (self.root / "two/Cargo.toml").symlink_to(self.root / "one/Cargo.toml")
        with self.assertRaises(evidence.EvidenceError):
            evidence.make_spdx(self.root)


class ToolchainInventoryTests(unittest.TestCase):
    def test_component_hash_binds_manifest_paths_and_file_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            sysroot = Path(temp)
            rustlib = sysroot / "lib/rustlib"
            rustlib.mkdir(parents=True)
            (rustlib / "components").write_text(
                "cargo-host\nrust-std-host\nrustc-host\n", encoding="utf-8")
            (sysroot / "bin").mkdir()
            (sysroot / "bin/rustc").write_bytes(b"compiler")
            (sysroot / "bin/cargo").write_bytes(b"cargo")
            (sysroot / "lib/rustlib/x86_64-unknown-linux-gnu/lib").mkdir(parents=True)
            (sysroot / "lib/rustlib/x86_64-unknown-linux-gnu/lib/libcore.rlib").write_bytes(
                b"standard library")
            (sysroot / "lib/librustc_driver.so").write_bytes(b"compiler backend")
            (sysroot / "lib/libLLVM.so").write_bytes(b"LLVM backend")
            (rustlib / "manifest-cargo-host").write_text("file:bin/cargo\n", encoding="utf-8")
            (rustlib / "manifest-rustc-host").write_text(
                "file:bin/rustc\nfile:lib/libLLVM.so\nfile:lib/librustc_driver.so\n",
                encoding="utf-8")
            (rustlib / "manifest-rust-std-host").write_text(
                "file:lib/rustlib/x86_64-unknown-linux-gnu/lib/libcore.rlib\n",
                encoding="utf-8")
            inventory = evidence._component_inventory(sysroot)
            self.assertEqual([item["name"] for item in inventory],
                             ["cargo-host", "rust-std-host", "rustc-host"])
            rustc_files = {row["path"]: row for row in inventory[2]["files"]}
            self.assertEqual(rustc_files["bin/rustc"]["sha256"], hashlib.sha256(b"compiler").hexdigest())
            self.assertEqual(rustc_files["lib/librustc_driver.so"]["sha256"],
                             hashlib.sha256(b"compiler backend").hexdigest())
            self.assertEqual(rustc_files["lib/libLLVM.so"]["sha256"],
                             hashlib.sha256(b"LLVM backend").hexdigest())
            std_file = inventory[1]["files"][0]
            self.assertEqual(std_file["bytes"], len(b"standard library"))
            digest = hashlib.sha256()
            for row in inventory[2]["files"]:
                digest.update(row["path"].encode("utf-8") + b"\0" + row["sha256"].encode("ascii") + b"\n")
            self.assertEqual(inventory[2]["tree_sha256"], digest.hexdigest())

    def test_component_inventory_refuses_symlinks_and_unknown_manifest_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            rustlib = root / "lib/rustlib"
            rustlib.mkdir(parents=True)
            (rustlib / "components").write_text("rustc-host\n", encoding="utf-8")
            (rustlib / "manifest-rustc-host").write_text("file:bin/rustc\n", encoding="utf-8")
            (root / "bin").mkdir()
            real = root / "bin/real-rustc"
            real.write_bytes(b"rustc")
            (root / "bin/rustc").symlink_to(real)
            with self.assertRaises(evidence.EvidenceError):
                evidence._component_inventory(root)
            (root / "bin/rustc").unlink()
            (root / "bin/rustc").write_bytes(b"rustc")
            (rustlib / "manifest-rustc-host").write_text("dir:bin\n", encoding="utf-8")
            with self.assertRaises(evidence.EvidenceError):
                evidence._component_inventory(root)

    def test_component_inventory_refuses_paths_resolving_outside_sysroot(self):
        with tempfile.TemporaryDirectory() as temp, tempfile.TemporaryDirectory() as outside:
            root = Path(temp)
            rustlib = root / "lib/rustlib"
            rustlib.mkdir(parents=True)
            (rustlib / "components").write_text("rustc-host\n", encoding="utf-8")
            (rustlib / "manifest-rustc-host").write_text("file:bin/rustc\n", encoding="utf-8")
            (root / "bin").mkdir()
            (Path(outside) / "rustc").write_bytes(b"external")
            (root / "bin/rustc").symlink_to(Path(outside) / "rustc")
            with self.assertRaises(evidence.EvidenceError):
                evidence._component_inventory(root)

    def test_component_count_and_file_bytes_are_bounded(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            rustlib = root / "lib/rustlib"
            rustlib.mkdir(parents=True)
            (rustlib / "components").write_text("rustc-host\n", encoding="utf-8")
            (rustlib / "manifest-rustc-host").write_text("file:" + "a" * 181 + "\n", encoding="utf-8")
            with self.assertRaises(evidence.EvidenceError):
                evidence._component_inventory(root)


class OutputTests(unittest.TestCase):
    def test_generation_emits_both_new_files_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "repo"
            root.mkdir()
            output = Path(temp) / "evidence"
            sbom = {"packages": [{"packageComment": "workspace member crate/Cargo.toml"}]}
            provenance = {"schema": "openatca.toolchain-provenance.v1"}
            with patch.object(evidence, "make_spdx", return_value=(sbom, 0)), \
                    patch.object(evidence, "make_provenance", return_value=provenance):
                self.assertEqual(evidence.generate(root, output, "1.89.0"), 0)
                self.assertEqual(json.loads((output / "sbom.spdx.json").read_text()), sbom)
                self.assertEqual(json.loads((output / "toolchain-provenance.json").read_text()), provenance)
                with self.assertRaises(FileExistsError):
                    evidence.generate(root, output, "1.89.0")

    def test_nonzero_third_party_count_is_emitted_then_fails_policy(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "repo"
            root.mkdir()
            output = Path(temp) / "evidence"
            with patch.object(evidence, "make_spdx", return_value=({"packages": []}, 1)), \
                    patch.object(evidence, "make_provenance", return_value={"schema": "test"}):
                with self.assertRaises(evidence.EvidenceError):
                    evidence.generate(root, output, "1.89.0")
            self.assertTrue((output / "sbom.spdx.json").is_file())
            self.assertTrue((output / "toolchain-provenance.json").is_file())


if __name__ == "__main__":
    unittest.main()
