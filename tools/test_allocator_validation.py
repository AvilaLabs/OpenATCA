"""Focused source-layout and evidence tests for the allocator acceptance oracle."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location(
    "allocator_validation", Path(__file__).with_name("allocator_validation.py"))
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def result(injected=False):
    return {
        "status": "FAIL_M3_ALLOCATION" if injected else "PASS_M3_EXERCISED",
        "injection_requested": injected,
        "fixture_ids": [
            "tracker_update_ooo_refusal_capacity_advance_flush",
            "fusion_adsb_hard_assoc_registration",
            "fusion_anonymous_init_and_assoc",
        ],
        "controls": {"alloc": 2, "zeroed": 1, "realloc": 1, "overflow": False},
        "tracker": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0,
            "overflow": False,
            "applied": 7, "refiltered": 2, "refused": 1, "capacity_refused": 1,
            "predictions": 4, "advance_rows": 3, "flush_rows": 2, "ended": 1, "rows": 8,
        },
        "fusion": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0,
            "overflow": False,
            "plots": 40, "hard_applied": 2, "candidates_confirmed": 3, "refused": 1,
            "rows": 20, "radar_tracks": 4, "registration_used": 1,
            "registration_history": 3, "anon_applied": 2, "local_rows": 5,
            "init_visits": 3, "init_tests": 2, "coast_rows": 4, "quiet_advance_rows": 3,
        },
    }


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def layout_tree(root):
    write(root / "Cargo.toml", '''
[workspace]
members = ["crates/oa-track"]
exclude = ["fuzz", "validation/oa-alloc-check"]
[workspace.lints.rust]
unsafe_code = "forbid"
''')
    write(root / "crates/oa-track/Cargo.toml", '''
[package]
name = "oa-track"
version = "0.1.0"
edition = "2021"
[lints]
workspace = true
''')
    write(root / "validation/oa-alloc-check/Cargo.toml", '''
[package]
name = "oa-alloc-check"
version = "0.1.0"
edition = "2021"
publish = false
[workspace]
[lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"
[dependencies]
oa-track = { path = "../../crates/oa-track" }
''')
    write(root / "validation/oa-alloc-check/src/main.rs", '''
#![deny(unsafe_code)]
fn main() {}
''')
    write(root / "validation/oa-alloc-check/src/allocator.rs", '''
#![allow(unsafe_code)]
#[global_allocator]
static A: System = System;
unsafe impl GlobalAlloc for A {}
''')


class AllocatorResultTests(unittest.TestCase):
    def test_exercised_and_injected_results_pass(self):
        self.assertEqual(gate.evaluate_allocator_result(json.dumps(result())), [])
        self.assertEqual(gate.evaluate_allocator_result(json.dumps(result(True)), True), [])

    def test_injected_mode_needs_alloc_activity_and_matching_status(self):
        value = result(True)
        value["tracker"]["alloc"] = 0
        self.assertTrue(any("tracker.alloc" in e for e in gate.evaluate_allocator_result(json.dumps(value), True)))
        value["tracker"]["alloc"] = 1
        value["status"] = "PASS_M3_EXERCISED"
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value), True))

    def test_missing_empty_wrong_class_and_duplicate_fixtures_fail_closed(self):
        self.assertTrue(gate.evaluate_allocator_result("{}"))
        self.assertTrue(gate.evaluate_allocator_result(""))
        value = result()
        value["fixture_ids"] = []
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        value["fixture_ids"] = ["tracker_update_ooo_refusal_capacity_advance_flush"] * 3
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))

    def test_bool_negative_missing_and_zero_coverage_are_not_integers_or_passes(self):
        value = result()
        value["controls"]["realloc"] = True
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        value = result()
        value["tracker"].pop("rows")
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        value = result()
        value["fusion"]["plots"] = 0
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))

    def test_overflow_partial_json_and_wrong_top_level_fail(self):
        value = result()
        value["overflow"] = True
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(result()) + " trailing"))
        self.assertTrue(gate.evaluate_allocator_result("[]"))


class AllocatorLayoutTests(unittest.TestCase):
    def test_standalone_harness_requires_the_strict_runtime_lints(self):
        for lint in ("unwrap_used", "expect_used", "panic", "indexing_slicing"):
            with self.subTest(lint=lint), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                layout_tree(root)
                manifest = root / "validation/oa-alloc-check/Cargo.toml"
                manifest.write_text(manifest.read_text().replace(f'{lint} = "deny"',
                                                                f'{lint} = "allow"'))
                self.assertTrue(any("runtime lints" in error
                                    for error in gate.allocation_layout_errors(root)))

    def test_realistic_isolated_layout_is_accepted(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            self.assertEqual(gate.allocation_layout_errors(root), [])

    def test_workspace_leak_and_harness_dependency_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            manifest = root / "Cargo.toml"
            manifest.write_text(manifest.read_text().replace('"crates/oa-track"', '"crates/oa-track", "validation/*"'), encoding="utf-8")
            self.assertTrue(any("workspace member" in e for e in gate.allocation_layout_errors(root)))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            write(root / "crates/oa-track/Cargo.toml", '''
[package]
name = "oa-track"
version = "0.1.0"
[lints]
workspace = true
[target.'cfg(unix)'.dependencies]
probe = { path = "../../validation/oa-alloc-check" }
''')
            self.assertTrue(any("points back" in e for e in gate.allocation_layout_errors(root)))

    def test_aliased_harness_dependency_and_outside_path_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            manifest = root / "validation/oa-alloc-check/Cargo.toml"
            manifest.write_text(manifest.read_text().replace('oa-track = {', 'track-alias = {'), encoding="utf-8")
            self.assertTrue(any("only the oa-track" in e for e in gate.allocation_layout_errors(root)))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            external = root.parent / (root.name + "-external")
            # The oracle must identify an escape from the path spelling before it attempts to read it.
            manifest = root / "validation/oa-alloc-check/Cargo.toml"
            manifest.write_text(manifest.read_text().replace("../../crates/oa-track", "../../../" + external.name), encoding="utf-8")
            with patch.object(gate, "_read_toml", wraps=gate._read_toml) as reader:
                self.assertTrue(gate.allocation_layout_errors(root))
                self.assertTrue(all(Path(call.args[0]).resolve().is_relative_to(root.resolve())
                                    for call in reader.call_args_list))

    def test_product_alias_cannot_depend_on_harness(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            manifest = root / "crates/oa-track/Cargo.toml"
            manifest.write_text(manifest.read_text() + '\n[dependencies]\ninnocuous = { package = "oa-alloc-check", path = "../../validation/oa-alloc-check" }\n')
            self.assertTrue(any("allocator harness" in e for e in gate.allocation_layout_errors(root)))

    def test_hashed_raw_strings_cannot_hide_a_local_unsafe_allowance(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            main = root / "validation/oa-alloc-check/src/main.rs"
            main.write_text(
                '#![deny(unsafe_code)]\nmod allocator;\nfn main() {\n'
                'let _ = r#"""#;\n'
                '#[allow(unsafe_code)] unsafe fn marker() { unsafe { std::ptr::read_volatile(&0u8); } }\n'
                '#[allow(unsafe_code)] fn call() { unsafe { marker(); } }\n'
                'let _ = "close"; call();\n}\n')
            self.assertTrue(any("main.rs" in e for e in gate.allocation_layout_errors(root)))

    def test_raw_literal_contents_are_opaque_and_unterminated_literals_fail(self):
        for prefix in ("r", "br", "cr"):
            for hashes in ("", "#", "###"):
                text = f'{prefix}{hashes}"unsafe #[allow(unsafe_code)]"{hashes};'
                self.assertNotIn("unsafe", gate._rust_tokens(text))
        with self.assertRaises(ValueError):
            gate._rust_tokens('r##"never closed')

    def test_guard_allows_allocator_unsafe_and_rejects_duplicate_attribute(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            write(root / "validation/oa-alloc-check/src/main.rs", '#![deny(unsafe_code)]\nfn main() { let _s = "unsafe { harmless }"; }\n')
            write(root / "validation/oa-alloc-check/src/allocator.rs", '#![allow(unsafe_code)]\n#[global_allocator]\nstatic A: System = System;\nunsafe impl GlobalAlloc for A { unsafe fn alloc(&self) { unsafe { work(); } } }\n')
            self.assertEqual(gate.allocation_layout_errors(root), [])
            write(root / "validation/oa-alloc-check/src/allocator.rs", '#![allow(unsafe_code)]\n#[global_allocator]\n#[global_allocator]\nstatic A: System = System;\nunsafe impl GlobalAlloc for A {}\n')
            self.assertTrue(any("exactly one global allocator" in e for e in gate.allocation_layout_errors(root)))


if __name__ == "__main__":
    unittest.main()
