from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

EXPECTED_SENTINEL_NAME = '.openatca-data-root'
EXPECTED_SENTINEL_BYTES = b'openatca-data-root-v1\n'
SCRIPT = Path(__file__).with_name('configure_test_data_root.py')
spec = importlib.util.spec_from_file_location('ci_test_data_root', SCRIPT)
helper = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = helper
spec.loader.exec_module(helper)

CHECKOUT = SCRIPT.resolve().parents[2]
GUARD_PATH = CHECKOUT / 'tools' / 'sealed_guard.py'
guard_spec = importlib.util.spec_from_file_location('ci_sealed_guard', GUARD_PATH)
sealed_guard = importlib.util.module_from_spec(guard_spec)
sys.modules[guard_spec.name] = sealed_guard
guard_spec.loader.exec_module(sealed_guard)


class ConfigureTestDataRootTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='openatca-ci-root-test-')
        self.runner_temp = Path(self.tmp.name) / 'runner-temp'
        self.runner_temp.mkdir(mode=0o700)
        self.env_file = self.runner_temp / 'github-env'
        self.env_file.write_bytes(b'PREEXISTING=kept\n')
        self.env_file.chmod(0o600)
        self.environment = {
            'RUNNER_TEMP': str(self.runner_temp),
            'GITHUB_ENV': str(self.env_file),
        }

    def tearDown(self):
        self.tmp.cleanup()

    def invoke(self, environment):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = helper.main(environment)
        receipt = json.loads(output.getvalue())
        return status, receipt

    def test_valid_root_is_consumed_by_real_guard_and_receipt_has_no_path(self):
        status, receipt = self.invoke(self.environment)
        self.assertEqual(status, 0)
        self.assertEqual(set(receipt), {
            'schema', 'status', 'sentinel', 'sentinel_bytes', 'sentinel_sha256'
        })
        self.assertEqual(receipt['status'], 'issued')
        self.assertEqual(receipt['sentinel'], EXPECTED_SENTINEL_NAME)
        self.assertEqual(receipt['sentinel_bytes'], len(EXPECTED_SENTINEL_BYTES))
        self.assertEqual(receipt['sentinel_sha256'], helper.hashlib.sha256(EXPECTED_SENTINEL_BYTES).hexdigest())
        appended = self.env_file.read_bytes()
        self.assertTrue(appended.startswith(b'PREEXISTING=kept\n'))
        new_line = appended[len(b'PREEXISTING=kept\n'):]
        self.assertTrue(new_line.startswith(b'OPENATCA_DATA='))
        self.assertTrue(new_line.endswith(b'\n'))
        self.assertEqual(new_line.count(b'\n'), 1)
        data_root = Path(new_line[len(b'OPENATCA_DATA='):-1].decode('utf-8'))
        self.assertEqual(data_root.parent, self.runner_temp)
        self.assertTrue(data_root.is_relative_to(self.runner_temp))
        self.assertEqual(data_root.stat().st_mode & 0o777, 0o700)
        marker = data_root / EXPECTED_SENTINEL_NAME
        self.assertEqual(marker.read_bytes(), EXPECTED_SENTINEL_BYTES)
        self.assertEqual(marker.stat().st_mode & 0o777, 0o600)
        self.assertEqual(set(item.name for item in data_root.iterdir()), {EXPECTED_SENTINEL_NAME})
        guard = sealed_guard.Guard(root=data_root)
        self.assertFalse(guard.broken)
        guard.check(data_root / 'ordinary-synthetic' / 'sample.bin')
        with self.assertRaises(sealed_guard.SealedError):
            guard.check(data_root / 'scds' / 'rec-20261119' / 'day2-synthetic' / 'sample.bin')

    def test_wrong_sentinel_name_and_bytes_fail_consumer_validation(self):
        wrong_root = self.runner_temp / 'wrong-root'
        wrong_root.mkdir(mode=0o700)
        wrong_name = wrong_root / '.openatca-data-root-v1'
        wrong_name.write_bytes(EXPECTED_SENTINEL_BYTES)
        wrong_name.chmod(0o600)
        self.assertTrue(sealed_guard.Guard(root=wrong_root).broken)
        wrong_bytes_root = self.runner_temp / 'wrong-bytes-root'
        wrong_bytes_root.mkdir(mode=0o700)
        wrong_bytes = wrong_bytes_root / EXPECTED_SENTINEL_NAME
        wrong_bytes.write_bytes(b'wrong-root-marker\n')
        wrong_bytes.chmod(0o600)
        self.assertTrue(sealed_guard.Guard(root=wrong_bytes_root).broken)

    def test_each_invocation_creates_a_unique_root(self):
        first_status, _ = self.invoke(self.environment)
        first_content = self.env_file.read_bytes()
        second_status, _ = self.invoke(self.environment)
        all_lines = self.env_file.read_bytes().splitlines()
        roots = [Path(line.split(b'=', 1)[1].decode('utf-8'))
                 for line in all_lines if line.startswith(b'OPENATCA_DATA=')]
        self.assertEqual((first_status, second_status), (0, 0))
        self.assertEqual(len(roots), 2)
        self.assertNotEqual(roots[0], roots[1])
        self.assertTrue(first_content.endswith(os.fsencode(str(roots[0])) + b'\n'))

    def test_missing_runner_temp_or_github_env_refuses_without_creation(self):
        before = set(self.runner_temp.iterdir())
        for environment in ({}, {'GITHUB_ENV': str(self.env_file)},
                            {'RUNNER_TEMP': str(self.runner_temp)}):
            status, receipt = self.invoke(environment)
            self.assertEqual(status, 2)
            self.assertEqual(receipt['status'], 'refused')
        self.assertEqual(set(self.runner_temp.iterdir()), before)

    def test_slash_and_checkout_runner_temp_refuse(self):
        for bad_temp in ('/', str(CHECKOUT)):
            environment = dict(self.environment, RUNNER_TEMP=bad_temp)
            before = self.env_file.read_bytes()
            status, receipt = self.invoke(environment)
            self.assertEqual(status, 2)
            self.assertEqual(receipt['status'], 'refused')
            self.assertEqual(self.env_file.read_bytes(), before)

    def test_wrong_github_env_location_and_missing_file_refuse(self):
        outside = Path(self.tmp.name) / 'outside-env'
        outside.write_bytes(b'')
        outside.chmod(0o600)
        for env_path in (str(outside), str(self.runner_temp / 'missing-env')):
            status, receipt = self.invoke(dict(self.environment, GITHUB_ENV=env_path))
            self.assertEqual(status, 2)
            self.assertEqual(receipt['status'], 'refused')
        self.assertEqual(outside.read_bytes(), b'')

    def test_symlinked_github_env_and_runner_temp_refuse(self):
        target = Path(self.tmp.name) / 'env-target'
        target.write_bytes(b'')
        target.chmod(0o600)
        link = self.runner_temp / 'env-link'
        link.symlink_to(target)
        status, receipt = self.invoke(dict(self.environment, GITHUB_ENV=str(link)))
        self.assertEqual((status, receipt['status']), (2, 'refused'))
        temp_link = Path(self.tmp.name) / 'temp-link'
        temp_link.symlink_to(self.runner_temp, target_is_directory=True)
        status, receipt = self.invoke(dict(self.environment, RUNNER_TEMP=str(temp_link)))
        self.assertEqual((status, receipt['status']), (2, 'refused'))
        self.assertEqual(target.read_bytes(), b'')

    def test_noncanonical_control_character_runner_temp_refuses(self):
        status, receipt = self.invoke(dict(self.environment, RUNNER_TEMP=str(self.runner_temp) + '\n'))
        self.assertEqual((status, receipt['status']), (2, 'refused'))
        self.assertFalse(any(path.name.startswith('openatca-data-root-')
                             for path in self.runner_temp.iterdir()))


if __name__ == '__main__':
    unittest.main()
