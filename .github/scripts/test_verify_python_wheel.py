from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile
from unittest import mock


SCRIPT_DIR = Path(__file__).parent
VERIFY_SPEC = importlib.util.spec_from_file_location("verify_python_wheel", SCRIPT_DIR / "verify_python_wheel.py")
if VERIFY_SPEC is None or VERIFY_SPEC.loader is None:
    raise RuntimeError("cannot load wheel verifier")
verify_python_wheel = importlib.util.module_from_spec(VERIFY_SPEC)
VERIFY_SPEC.loader.exec_module(verify_python_wheel)

BACKEND_PATH = SCRIPT_DIR.parents[1] / "python" / "openatca_build.py"
BACKEND_SPEC = importlib.util.spec_from_file_location("openatca_build_for_verifier_test", BACKEND_PATH)
if BACKEND_SPEC is None or BACKEND_SPEC.loader is None:
    raise RuntimeError("cannot load wheel backend")
openatca_build = importlib.util.module_from_spec(BACKEND_SPEC)
BACKEND_SPEC.loader.exec_module(openatca_build)


class WheelVerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.dist = Path(self.temp.name) / "dist"
        self.dist.mkdir()
        self.wheel = self.dist / verify_python_wheel.WHEEL_NAME
        self.wheel.write_bytes(openatca_build._wheel())

    def test_accepts_current_backend_wheel(self) -> None:
        verify_python_wheel.verify(self.dist)

    def test_rejects_unexpected_extra_artifact(self) -> None:
        (self.dist / "unexpected.txt").write_text("extra", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "exactly the expected wheel"):
            verify_python_wheel.verify(self.dist)

    def test_rejects_changed_member_without_matching_record(self) -> None:
        with zipfile.ZipFile(self.wheel) as archive:
            contents = {name: archive.read(name) for name in archive.namelist()}
        target = "openatca/__init__.py"
        contents[target] += b"\n# tampered\n"
        self.wheel.unlink()
        with zipfile.ZipFile(self.wheel, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, payload in contents.items():
                archive.writestr(name, payload)
        with self.assertRaisesRegex(ValueError, "RECORD hashes or sizes"):
            verify_python_wheel.verify(self.dist)

    def _rewrite_member(self, name: str, transform) -> None:
        with zipfile.ZipFile(self.wheel) as archive:
            contents = {member: archive.read(member) for member in archive.namelist()}
        contents[name] = transform(contents[name])
        self.wheel.unlink()
        with zipfile.ZipFile(self.wheel, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for member, payload in contents.items():
                archive.writestr(member, payload)

    def test_rejects_oversized_compressed_file_before_zip_parse(self) -> None:
        with self.wheel.open("r+b") as stream:
            stream.truncate(verify_python_wheel.MAX_WHEEL_BYTES + 1)
        with mock.patch.object(verify_python_wheel.zipfile, "ZipFile") as zip_open:
            with self.assertRaisesRegex(ValueError, "compressed wheel exceeds"):
                verify_python_wheel.verify(self.dist)
        zip_open.assert_not_called()

    def test_rejects_duplicate_or_conflicting_metadata_headers(self) -> None:
        metadata_name = "openatca-0.1.0.dist-info/METADATA"
        self._rewrite_member(
            metadata_name,
            lambda payload: payload.replace(
                b"Name: openatca\n", b"Name: impostor\nName: openatca\n", 1
            ),
        )
        with self.assertRaisesRegex(ValueError, "header Name"):
            verify_python_wheel.verify(self.dist)

    def test_rejects_identity_spoof_lines_in_metadata_body(self) -> None:
        metadata_name = "openatca-0.1.0.dist-info/METADATA"

        def body_spoof(payload: bytes) -> bytes:
            headers, body = payload.split(b"\n\n", 1)
            headers = headers.replace(b"Name: openatca\n", b"")
            headers = headers.replace(b"Version: 0.1.0\n", b"")
            return headers + b"\n\nName: openatca\nVersion: 0.1.0\n" + body

        self._rewrite_member(metadata_name, body_spoof)
        with self.assertRaisesRegex(ValueError, "header Name"):
            verify_python_wheel.verify(self.dist)


if __name__ == "__main__":
    unittest.main()
