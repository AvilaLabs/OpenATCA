"""Synthetic gates for P-010 build receipt parsing; no Cargo build or input data is used."""
from __future__ import annotations

import importlib.util
import os
import tempfile
import unittest
from unittest import mock
from pathlib import Path

SCRIPT = Path(__file__).with_name("build_p010_receipts.py")
SPEC = importlib.util.spec_from_file_location("build_p010_receipts_under_test", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load P-010 receipt producer source")
producer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(producer)


class P010ReceiptSourceTests(unittest.TestCase):
    # REQ: OA-TRK-412
    def test_strict_json_refuses_duplicate_keys_and_nonfinite_values(self) -> None:
        with self.assertRaises(producer.Refused):
            producer._json_bytes(b'{"role":"candidate","role":"replication"}', "fixture")
        with self.assertRaises(producer.Refused):
            producer._json_bytes(b'{"fresh":NaN}', "fixture")
        with self.assertRaises(producer.Refused):
            producer._json_bytes(b'{"size":1e999}', "fixture")
        deep = b"[" * 1200 + b"0" + b"]" * 1200
        with self.assertRaises(producer.Refused):
            producer._json_bytes(deep, "fixture")

    # REQ: OA-TRK-412
    def test_local_dependency_graph_is_checkout_bound_and_transitive(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "checkout"
            app_manifest = root / "crates/app/Cargo.toml"
            dep_manifest = root / "crates/dep/Cargo.toml"
            app_manifest.parent.mkdir(parents=True)
            dep_manifest.parent.mkdir(parents=True)
            app_manifest.write_text("[package]\nname='app'\n", encoding="utf-8")
            dep_manifest.write_text("[package]\nname='dep'\n", encoding="utf-8")
            app_id = "path+file:///checkout#app@1.0.0"
            dep_id = "path+file:///checkout#dep@1.0.0"
            metadata = {
                "packages": [
                    {"id": app_id, "name": "app", "source": None, "manifest_path": str(app_manifest.resolve())},
                    {"id": dep_id, "name": "dep", "source": None, "manifest_path": str(dep_manifest.resolve())},
                ],
                "resolve": {"nodes": [
                    {"id": app_id, "deps": [{"pkg": dep_id}]},
                    {"id": dep_id, "deps": []},
                ]},
            }
            packages, reachable = producer._package_graph(metadata, root.resolve(), "app", app_manifest.resolve())
            self.assertEqual(set(packages), {app_id, dep_id})
            self.assertEqual(reachable, {app_id, dep_id})

    # REQ: OA-TRK-412
    def test_local_dependency_graph_refuses_foreign_local_path_and_duplicate_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "checkout"
            foreign = base / "foreign/Cargo.toml"
            app_manifest = root / "crates/app/Cargo.toml"
            app_manifest.parent.mkdir(parents=True)
            foreign.parent.mkdir(parents=True)
            app_manifest.write_text("[package]\nname='app'\n", encoding="utf-8")
            foreign.write_text("[package]\nname='foreign'\n", encoding="utf-8")
            app_id = "path+file:///checkout#app@1.0.0"
            foreign_id = "path+file:///foreign#foreign@1.0.0"
            outside = {
                "packages": [
                    {"id": app_id, "name": "app", "source": None, "manifest_path": str(app_manifest.resolve())},
                    {"id": foreign_id, "name": "foreign", "source": None, "manifest_path": str(foreign.resolve())},
                ],
                "resolve": {"nodes": [{"id": app_id, "deps": []}]},
            }
            with self.assertRaises(producer.Refused):
                producer._package_graph(outside, root.resolve(), "app", app_manifest.resolve())

            duplicate = {
                "packages": [
                    {"id": app_id, "name": "app", "source": None, "manifest_path": str(app_manifest.resolve())},
                    {"id": app_id, "name": "app", "source": None, "manifest_path": str(app_manifest.resolve())},
                ],
                "resolve": {"nodes": [{"id": app_id, "deps": []}]},
            }
            with self.assertRaises(producer.Refused):
                producer._package_graph(duplicate, root.resolve(), "app", app_manifest.resolve())

    # REQ: OA-TRK-412
    def test_cargo_artifact_hashing_is_target_local_and_descriptor_bound(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "role-target"
            artifact = target / "release/deps/liblocal.rlib"
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(b"synthetic compiled artifact")
            record, size = producer._hash_cargo_file(str(artifact), target.resolve(), "fixture", set())
            self.assertEqual(record["bytes"], size)
            self.assertEqual(record["path"], str(artifact))
            external = root / "foreign.rlib"
            external.write_bytes(b"synthetic foreign artifact")
            with self.assertRaises(producer.Refused):
                producer._hash_cargo_file(str(external), target.resolve(), "fixture", set())

    # REQ: OA-TRK-412
    def test_target_inventory_counts_are_bounded_and_include_empty_directories(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target"
            (target / "release/deps/empty").mkdir(parents=True)
            (target / "release/deps/artifact.rmeta").write_bytes(b"x")
            usage = producer._measure_target(target)
            self.assertEqual((usage["files"], usage["directories"], usage["bytes"]), (1, 4, 1))
            old_cap = producer.MAX_TARGET_FILES
            try:
                producer.MAX_TARGET_FILES = 0
                with self.assertRaises(producer.Refused):
                    producer._measure_target(target)
            finally:
                producer.MAX_TARGET_FILES = old_cap

    # REQ: OA-TRK-412
    def test_caller_path_shim_is_not_selected_or_executed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            shim_dir = Path(directory) / "shim"
            shim_dir.mkdir()
            marker = Path(directory) / "shim-executed"
            shim = shim_dir / "cargo"
            shim.write_text(f"#!/bin/sh\ntouch {marker}\n", encoding="utf-8")
            shim.chmod(0o700)
            with mock.patch.dict(os.environ, {"PATH": str(shim_dir)}):
                self.assertEqual(producer._shell_environment()["PATH"], producer.APPROVED_PATH)
                build_argv = producer.p010._build_argv("candidate", Path(directory) / "target")
                self.assertEqual(build_argv[0], producer.p010.CARGO_EXECUTABLE)
                self.assertNotEqual(build_argv[0], str(shim))
            self.assertFalse(marker.exists())


if __name__ == "__main__":
    unittest.main()
