"""Keep the provisional global status-code map identical across C, Rust, and ctypes."""
from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HEADER = ROOT / "include/openatca.h"
RUST = ROOT / "crates/openatca-core-ffi/src/ffi.rs"
NOTAM_RUST = ROOT / "crates/openatca-core-ffi/src/formats/notam.rs"
PYTHON = ROOT / "python/openatca/__init__.py"


def read_statuses() -> tuple[dict[str, int], dict[str, int], dict[str, int]]:
    header_text = HEADER.read_text(encoding="utf-8")
    rust_text = RUST.read_text(encoding="utf-8") + "\n" + NOTAM_RUST.read_text(encoding="utf-8")
    python_text = PYTHON.read_text(encoding="utf-8")
    header = {
        name: int(code)
        for name, code in re.findall(r"\b(OA_STATUS_[A-Z0-9_]+)\s*=\s*(\d+)\s*,?", header_text)
    }
    rust_constants = dict(re.findall(
        r"\bpub const ([A-Z0-9_]+):\s*u32\s*=\s*([A-Z0-9_]+)\s*;", rust_text
    ))
    rust_constants.update({
        name: code for name, code in re.findall(
            r"\bpub const ([A-Z0-9_]+):\s*u32\s*=\s*(\d+)\s*;", rust_text
        )
    })

    def resolve(name: str, seen: set[str] | None = None) -> int:
        seen = set() if seen is None else seen
        if name in seen or name not in rust_constants:
            raise ValueError(f"unresolved Rust status constant: {name}")
        seen.add(name)
        value = rust_constants[name]
        return int(value) if value.isdigit() else resolve(value, seen)

    rust = {
        name: resolve(name)
        for name in rust_constants
        if name.startswith("OA_STATUS_")
    }
    start = python_text.index("_STATUS_NAMES: Final[dict[int, str]] = {")
    end = python_text.index("\n}", start)
    python = {
        f"OA_STATUS_{name}": int(code)
        for code, name in re.findall(r"^\s*(\d+):\s*\"([A-Z0-9_]+)\",\s*$", python_text[start:end], re.MULTILINE)
    }
    return header, rust, python


class StatusRegistryTests(unittest.TestCase):
    def test_c_rust_and_python_status_names_and_numbers_are_unique_and_identical(self) -> None:
        header, rust, python = read_statuses()
        self.assertTrue(header)
        self.assertEqual(header, rust)
        self.assertEqual(header, python)
        for registry in (header, rust, python):
            self.assertEqual(len(registry), len(set(registry)))
            self.assertEqual(len(registry.values()), len(set(registry.values())))
        self.assertEqual(header["OA_STATUS_NOTAM_MEMORY_BUDGET"], 48)
        self.assertEqual(header["OA_STATUS_GRIB2_FORMAT"], 80)
        self.assertEqual(header["OA_STATUS_GRIB2_UNSUPPORTED"], 81)
        self.assertEqual(header["OA_STATUS_GRIB2_LIMIT"], 82)


if __name__ == "__main__":
    unittest.main()
