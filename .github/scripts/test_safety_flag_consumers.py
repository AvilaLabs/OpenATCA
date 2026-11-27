from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_safety_flag_consumers.py")
SPEC = importlib.util.spec_from_file_location("safety_flag_consumers", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable to load safety flag checker")
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


class SafetyFlagConsumerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        (self.root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        self.provider = self.root / "crates" / "oa-safety"
        self.consumer = self.root / "crates" / "consumer"
        (self.provider / "src").mkdir(parents=True)
        (self.consumer / "src").mkdir(parents=True)
        (self.provider / "Cargo.toml").write_text(
            '[package]\nname="oa-safety"\nversion="0.1.0"\n', encoding="utf-8"
        )
        self.consumer_manifest = self.consumer / "Cargo.toml"
        self.consumer_manifest.write_text(
            '[package]\nname="consumer"\nversion="0.1.0"\n'
            '[dependencies]\nsafety_dep={package="oa-safety",path="../oa-safety"}\n',
            encoding="utf-8",
        )
        self.provider_source = self.provider / "src" / "lib.rs"
        self.provider_source.write_text(
            "/// A bit in this row's public flags.\n"
            "pub const F_ACTIVE: u8 = 1;\n"
            "/// Another flag bit.\n"
            "pub const F_VISIBLE: u8 = 1 << 1;\n"
            "pub const F_ALL_FLAGS: u8 = F_ACTIVE | F_VISIBLE;\n"
            "pub mod flags { pub use super::{F_ACTIVE, F_VISIBLE, F_ALL_FLAGS}; }\n"
            "/// `Measurement::presence`: bit 2: caller verified a common source epoch.\n"
            "pub const INPUT_PRESENT: u8 = 4;\n"
            "pub const NOT_A_FLAG: u8 = 7;\n",
            encoding="utf-8",
        )
        for package_name in CHECKER.SAFETY_PACKAGES - {"oa-safety", "oa-adapt"}:
            package_root = self.root / "crates" / package_name
            (package_root / "src").mkdir(parents=True)
            (package_root / "Cargo.toml").write_text(
                f'[package]\nname="{package_name}"\nversion="0.1.0"\n', encoding="utf-8"
            )
            (package_root / "src" / "lib.rs").write_text("", encoding="utf-8")
        adapt_root = self.root / "tools" / "oa-adapt"
        (adapt_root / "src").mkdir(parents=True)
        (adapt_root / "Cargo.toml").write_text(
            '[package]\nname="oa-adapt"\nversion="0.1.0"\n', encoding="utf-8"
        )
        (adapt_root / "src" / "lib.rs").write_text("", encoding="utf-8")
        self.consumer_source = self.consumer / "src" / "lib.rs"
        self.allowlist = self.root / ".github" / "safety-flag-consumer-allowlist.json"
        self.allowlist.parent.mkdir()
        self.allowlist.write_text('{"version":1,"entries":[]}', encoding="utf-8")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def findings(self) -> tuple[list[object], list[object]]:
        return CHECKER.check(self.root, self.allowlist)

    def write_consumer(self, source: str) -> None:
        self.consumer_source.write_text(source, encoding="utf-8")

    def test_unconsumed_flags_are_reported_and_old_code_would_fail(self) -> None:
        with self.provider_source.open("a", encoding="utf-8") as source:
            source.write("fn only_internal() { let _ = F_ACTIVE | F_VISIBLE; }\n")
        self.write_consumer("pub fn unrelated() {}\n")
        violations, stale = self.findings()
        self.assertEqual(
            {item.flag.name for item in violations},
            {"F_ACTIVE", "F_VISIBLE", "F_ALL_FLAGS", "INPUT_PRESENT"},
        )
        self.assertFalse(stale)

    def test_qualified_use_and_import_alias_are_real_consumers(self) -> None:
        self.write_consumer(
            "use safety_dep as safety;\n"
            "use safety::F_ACTIVE as ACTIVE;\n"
            "use safety_dep::flags as flags_api;\n"
            "pub fn apply(bits: u8) -> bool {\n"
            "    bits & ACTIVE != 0 || bits & safety::F_VISIBLE != 0 || bits & flags_api::F_ALL_FLAGS != 0 || bits & safety::INPUT_PRESENT != 0\n"
            "}\n"
        )
        violations, _ = self.findings()
        self.assertFalse(violations)

    def test_import_declaration_without_a_code_read_is_not_a_consumer(self) -> None:
        self.write_consumer("use safety_dep::F_ACTIVE;\npub fn unrelated() {}\n")
        violations, _ = self.findings()
        self.assertEqual(
            {item.flag.name for item in violations},
            {"F_ACTIVE", "F_VISIBLE", "F_ALL_FLAGS", "INPUT_PRESENT"},
        )

    def test_real_downstream_use_through_public_reexport_counts(self) -> None:
        middle = self.root / "crates" / "oa-track"
        middle_manifest = middle / "Cargo.toml"
        middle_manifest.write_text(
            '[package]\nname="oa-track"\nversion="0.1.0"\n'
            '[dependencies]\nsafety_dep={package="oa-safety",path="../oa-safety"}\n',
            encoding="utf-8",
        )
        (middle / "src" / "lib.rs").write_text(
            "pub use safety_dep::F_ACTIVE as EXPORTED_ACTIVE;\n", encoding="utf-8"
        )
        self.consumer_manifest.write_text(
            '[package]\nname="consumer"\nversion="0.1.0"\n'
            '[dependencies]\ntracker={package="oa-track",path="../oa-track"}\n',
            encoding="utf-8",
        )
        # A re-export on its own is not an actual consumer.
        self.write_consumer("pub fn unrelated() {}\n")
        violations, _ = self.findings()
        self.assertEqual(
            {item.flag.name for item in violations},
            {"F_ACTIVE", "F_VISIBLE", "F_ALL_FLAGS", "INPUT_PRESENT"},
        )
        self.write_consumer(
            "use tracker::EXPORTED_ACTIVE as ACTIVE;\n"
            "pub fn apply(bits: u8) -> bool { bits & ACTIVE != 0 }\n"
        )
        violations, _ = self.findings()
        self.assertEqual(
            {item.flag.name for item in violations},
            {"F_VISIBLE", "F_ALL_FLAGS", "INPUT_PRESENT"},
        )

    def test_comments_strings_and_cfg_test_only_references_do_not_count(self) -> None:
        self.write_consumer(
            '// safety_dep::F_ACTIVE\n'
            'const NOTE: &str = "F_ACTIVE safety_dep::F_VISIBLE";\n'
            '#[cfg(test)] mod tests {\n'
            '    use safety_dep::{F_ACTIVE, F_VISIBLE};\n'
            '    fn test_only() { let _ = F_ACTIVE | F_VISIBLE; }\n'
            '}\n'
            '#[cfg(test)] fn standalone_test_only() { let _ = safety_dep::F_ACTIVE; }\n'
        )
        violations, _ = self.findings()
        self.assertEqual(
            {item.flag.name for item in violations},
            {"F_ACTIVE", "F_VISIBLE", "F_ALL_FLAGS", "INPUT_PRESENT"},
        )

    def test_exact_allowlist_entry_suppresses_one_flag_and_stale_entry_fails(self) -> None:
        self.write_consumer("pub fn unrelated() {}\n")
        entries = [
            {
                "package": "oa-safety",
                "constant": "F_ACTIVE",
                "declaration": "crates/oa-safety/src/lib.rs",
                "reason": "Public compatibility bit intentionally retained pending API removal review.",
            }
        ]
        self.allowlist.write_text(json.dumps({"version": 1, "entries": entries}), encoding="utf-8")
        # The declaration is in lib.rs and the exception is exact, so only the other flag fails.
        violations, stale = self.findings()
        self.assertEqual(
            set(item.flag.name for item in violations),
            {"F_VISIBLE", "F_ALL_FLAGS", "INPUT_PRESENT"},
        )
        self.assertFalse(stale)
        self.provider_source.write_text("pub const NOT_A_FLAG: u8 = 7;\n", encoding="utf-8")
        violations, stale = self.findings()
        self.assertFalse(violations)
        self.assertEqual(stale, [("oa-safety", "F_ACTIVE", "crates/oa-safety/src/lib.rs")])

    def test_missing_or_wrong_configuration_fails_closed(self) -> None:
        with self.assertRaises(ValueError):
            CHECKER.check(self.root / "wrong-root", self.allowlist)
        with self.assertRaises(ValueError):
            CHECKER.check(Path("/"), self.allowlist)
        missing_package_manifest = self.root / "crates" / "oa-track" / "Cargo.toml"
        with tempfile.TemporaryDirectory() as backup_dir:
            backup = Path(backup_dir) / "Cargo.toml"
            missing_package_manifest.replace(backup)
            try:
                with self.assertRaises(ValueError):
                    self.findings()
            finally:
                backup.replace(missing_package_manifest)
        self.allowlist.unlink()
        with self.assertRaises(ValueError):
            self.findings()


if __name__ == "__main__":
    unittest.main()
