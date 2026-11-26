from __future__ import annotations

import base64
import csv
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from unittest import mock


PYTHON_ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("openatca_build", PYTHON_ROOT / "openatca_build.py")
assert SPEC is not None and SPEC.loader is not None
backend = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(backend)


class WheelBackendTests(unittest.TestCase):
    def build(self, directory: Path) -> bytes:
        filename = backend.build_wheel(str(directory))
        return (directory / filename).read_bytes()

    def test_inventory_metadata_and_record_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            wheel_path = Path(temp)
            data = self.build(wheel_path)
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            names = set(archive.namelist())
            expected = {
                "openatca/__init__.py",
                "openatca/py.typed",
                f"{backend.DIST_INFO}/METADATA",
                f"{backend.DIST_INFO}/WHEEL",
                f"{backend.DIST_INFO}/licenses/LICENSE",
                f"{backend.DIST_INFO}/RECORD",
            }
            self.assertEqual(names, expected)
            metadata = archive.read(f"{backend.DIST_INFO}/METADATA").decode("utf-8")
            self.assertIn("Name: openatca\n", metadata)
            self.assertIn("Version: 0.1.0\n", metadata)
            self.assertIn("Requires-Python: >=3.10\n", metadata)
            self.assertIn("License-Expression: Apache-2.0\n", metadata)
            self.assertNotIn("Requires-Dist:", metadata)
            wheel_info = archive.read(f"{backend.DIST_INFO}/WHEEL").decode("ascii")
            self.assertIn("Root-Is-Purelib: true\n", wheel_info)
            self.assertIn("Tag: py3-none-any\n", wheel_info)
            records = list(csv.reader(io.StringIO(
                archive.read(f"{backend.DIST_INFO}/RECORD").decode("utf-8"), newline=""
            )))
            self.assertEqual(len(records), len(expected))
            seen = set()
            for name, digest, size in records:
                self.assertIn(name, expected)
                self.assertNotIn(name, seen)
                seen.add(name)
                if name.endswith("/RECORD"):
                    self.assertEqual((digest, size), ("", ""))
                    continue
                payload = archive.read(name)
                encoded = base64.urlsafe_b64encode(hashlib.sha256(payload).digest()).rstrip(b"=").decode("ascii")
                self.assertEqual(digest, f"sha256={encoded}")
                self.assertEqual(int(size), len(payload))
            self.assertEqual(seen, expected)
            self.assertNotIn("crates/", names)
            self.assertFalse(any("fixture" in name for name in names))

    def test_deterministic_wheel_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            self.assertEqual(self.build(Path(first)), self.build(Path(second)))

    def test_refuses_overwrite_and_preserves_existing_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            target = directory / backend.WHEEL_FILENAME
            target.write_bytes(b"keep existing")
            with self.assertRaises(FileExistsError):
                backend.build_wheel(str(directory))
            self.assertEqual(target.read_bytes(), b"keep existing")

    def test_rejects_missing_file_and_symlink_output_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with self.assertRaises(ValueError):
                backend.build_wheel(str(root / "missing"))
            target = root / "real"
            target.mkdir()
            link = root / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaises(ValueError):
                backend.build_wheel(str(link))

    def test_rejects_symlinked_source_parent_component(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            real = root / "real"
            real.mkdir()
            (real / "python").mkdir()
            (real / "python" / "openatca_build.py").write_text("", encoding="utf-8")
            alias = root / "alias"
            alias.symlink_to(real, target_is_directory=True)
            fake_backend = alias / "python" / "openatca_build.py"
            with mock.patch.object(backend, "__file__", str(fake_backend)):
                with self.assertRaises(ValueError):
                    backend._read_source("README.md")

    def test_fifo_package_input_refuses_without_blocking(self) -> None:
        if not hasattr(os, "mkfifo"):
            self.skipTest("FIFO creation is unavailable on this platform")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            module = root / "openatca_build.py"
            shutil.copyfile(PYTHON_ROOT / "openatca_build.py", module)
            os.mkfifo(root / "input.fifo")
            code = (
                "import importlib.util,sys; "
                "spec=importlib.util.spec_from_file_location('backend',sys.argv[1]); "
                "module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module); "
                "try_read=False; "
                "exec(\"try:\\n module._read_source('input.fifo')\\n"
                "except ValueError:\\n try_read=True\"); "
                "sys.exit(0 if try_read else 7)"
            )
            result = subprocess.run(
                [sys.executable, "-c", code, str(module)],
                capture_output=True,
                timeout=2,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", "replace"))

    def test_create_new_write_failure_preserves_replacement_name(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            target = directory / backend.WHEEL_FILENAME
            replacement = b"concurrent replacement must survive"
            original_fdopen = backend.os.fdopen

            def replace_then_fail(fd, mode, closefd=False):
                if mode == "rb":
                    return original_fdopen(fd, mode, closefd=closefd)
                target.unlink()
                target.write_bytes(replacement)
                raise OSError("injected write setup failure")

            with mock.patch.object(backend.os, "fdopen", side_effect=replace_then_fail):
                with self.assertRaisesRegex(OSError, "injected write setup failure"):
                    backend.build_wheel(str(directory))
            self.assertEqual(target.read_bytes(), replacement)

    def test_metadata_write_failure_retains_partial_create_new_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            target = directory / "METADATA"
            original_fdopen = backend.os.fdopen
            try:
                class PartialWriter:
                    def __init__(self, fd, _mode, closefd=False):
                        self.stream = original_fdopen(fd, "wb", closefd=closefd)

                    def __enter__(self):
                        return self

                    def __exit__(self, *_args):
                        return False

                    def write(self, data):
                        self.stream.write(data[:1])
                        self.stream.flush()
                        raise OSError("injected partial write")

                    def flush(self):
                        self.stream.flush()

                    def fileno(self):
                        return self.stream.fileno()

                with mock.patch.object(backend.os, "fdopen", side_effect=PartialWriter):
                    with self.assertRaisesRegex(OSError, "injected partial write"):
                        backend._write_new_at(directory_fd, "METADATA", b"metadata")
            finally:
                os.close(directory_fd)
            self.assertEqual(target.read_bytes(), b"m")


if __name__ == "__main__":
    unittest.main()
