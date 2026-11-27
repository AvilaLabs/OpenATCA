#!/usr/bin/env python3
"""Create a synthetic CI data root and export its path for this job step."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile

DATA_ROOT_SENTINEL = '.openatca-data-root'
DATA_ROOT_SENTINEL_BYTES = b'openatca-data-root-v1\n'
MAX_ENV_LINE_BYTES = 8192
NOFOLLOW = getattr(os, 'O_NOFOLLOW', None)
CLOEXEC = getattr(os, 'O_CLOEXEC', 0)


def _canonical_path(value: str, label: str) -> Path:
    if not isinstance(value, str) or not value or '\0' in value:
        raise ValueError(f'{label} is missing or invalid')
    if any(ord(char) < 0x20 or ord(char) == 0x7f for char in value):
        raise ValueError(f'{label} contains a control character')
    path = Path(value)
    if not path.is_absolute():
        raise ValueError(f'{label} must be absolute')
    try:
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError) as exc:
        raise ValueError(f'{label} does not resolve') from exc
    if path != resolved:
        raise ValueError(f'{label} must use its canonical path')
    return path


def _owned_directory(path: Path, label: str, uid: int) -> None:
    try:
        info = os.lstat(path)
    except OSError as exc:
        raise ValueError(f'{label} is unavailable') from exc
    if (stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode)
            or info.st_uid != uid):
        raise ValueError(f'{label} must be an owner-owned real directory')


def _validated_runner_temp(value: str, checkout: Path, uid: int) -> Path:
    path = _canonical_path(value, 'RUNNER_TEMP')
    _owned_directory(path, 'RUNNER_TEMP', uid)
    if uid == 0:
        raise ValueError('root-owned runner execution is refused')
    if path == checkout or path.is_relative_to(checkout) or checkout.is_relative_to(path):
        raise ValueError('RUNNER_TEMP must be separate from the checkout')
    return path


def _validated_env_file(value: str, runner_temp: Path, uid: int):
    if NOFOLLOW is None:
        raise ValueError('platform cannot open GITHUB_ENV without following symlinks')
    path = _canonical_path(value, 'GITHUB_ENV')
    if path == runner_temp or not path.is_relative_to(runner_temp):
        raise ValueError('GITHUB_ENV must be a file below RUNNER_TEMP')
    try:
        path_info = os.lstat(path)
    except OSError as exc:
        raise ValueError('GITHUB_ENV is unavailable') from exc
    if (stat.S_ISLNK(path_info.st_mode) or not stat.S_ISREG(path_info.st_mode)
            or path_info.st_uid != uid):
        raise ValueError('GITHUB_ENV must be an owner-owned regular non-symlink file')
    flags = os.O_WRONLY | os.O_APPEND | NOFOLLOW | CLOEXEC
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise ValueError('GITHUB_ENV could not be opened safely') from exc
    opened = os.fstat(fd)
    if (not stat.S_ISREG(opened.st_mode) or opened.st_uid != uid
            or (opened.st_dev, opened.st_ino) != (path_info.st_dev, path_info.st_ino)):
        os.close(fd)
        raise ValueError('GITHUB_ENV changed during validation')
    return path, fd, (opened.st_dev, opened.st_ino)


def _validate_fixture(data_root: Path, runner_temp: Path, uid: int) -> str:
    if data_root.parent != runner_temp or not data_root.name.startswith('openatca-data-root-'):
        raise ValueError('synthetic data root is outside RUNNER_TEMP')
    if data_root.resolve(strict=True) != data_root:
        raise ValueError('synthetic data root is not canonical')
    _owned_directory(data_root, 'synthetic data root', uid)
    if stat.S_IMODE(os.lstat(data_root).st_mode) != 0o700:
        raise ValueError('synthetic data root must have mode 0700')
    marker = data_root / DATA_ROOT_SENTINEL
    if NOFOLLOW is None:
        raise ValueError('platform cannot validate a non-symlink sentinel')
    try:
        fd = os.open(marker, os.O_RDONLY | NOFOLLOW | CLOEXEC)
    except OSError as exc:
        raise ValueError('synthetic data root sentinel is missing or unsafe') from exc
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != uid
                or stat.S_IMODE(info.st_mode) != 0o600
                or info.st_size != len(DATA_ROOT_SENTINEL_BYTES)):
            raise ValueError('synthetic data root sentinel has invalid type, owner, mode, or size')
        contents = os.read(fd, len(DATA_ROOT_SENTINEL_BYTES) + 1)
        if contents != DATA_ROOT_SENTINEL_BYTES:
            raise ValueError('synthetic data root sentinel content is invalid')
    finally:
        os.close(fd)
    if {entry.name for entry in data_root.iterdir()} != {DATA_ROOT_SENTINEL}:
        raise ValueError('synthetic data root must be empty except for its sentinel')
    return hashlib.sha256(DATA_ROOT_SENTINEL_BYTES).hexdigest()


def _create_data_root(runner_temp: Path, uid: int) -> Path:
    raw = tempfile.mkdtemp(prefix='openatca-data-root-', dir=runner_temp)
    root = Path(raw)
    try:
        os.chmod(root, 0o700)
        if NOFOLLOW is None:
            raise ValueError('platform cannot create a non-symlink sentinel')
        marker = root / DATA_ROOT_SENTINEL
        fd = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL | NOFOLLOW | CLOEXEC, 0o600)
        try:
            view = memoryview(DATA_ROOT_SENTINEL_BYTES)
            while view:
                written = os.write(fd, view)
                if written <= 0:
                    raise OSError('short sentinel write')
                view = view[written:]
            os.fchmod(fd, 0o600)
            os.fsync(fd)
        finally:
            os.close(fd)
        _validate_fixture(root, runner_temp, uid)
        return root
    except BaseException:
        try:
            marker_path = root / DATA_ROOT_SENTINEL
            marker_path.unlink(missing_ok=True)
            root.rmdir()
        except OSError:
            pass
        raise


def configure_test_data_root(environment: dict[str, str]) -> dict[str, object]:
    runner_value = environment.get('RUNNER_TEMP')
    env_file_value = environment.get('GITHUB_ENV')
    if not runner_value or not env_file_value:
        raise ValueError('RUNNER_TEMP and GITHUB_ENV are both required')
    checkout = Path(__file__).resolve().parents[2]
    uid = os.getuid()
    runner_temp = _validated_runner_temp(runner_value, checkout, uid)
    env_file, fd, identity = _validated_env_file(env_file_value, runner_temp, uid)
    root = None
    try:
        root = _create_data_root(runner_temp, uid)
        value = str(root)
        if any(ord(char) < 0x20 or ord(char) == 0x7f for char in value):
            raise ValueError('synthetic data root contains a control character')
        line = f'OPENATCA_DATA={value}\n'.encode('utf-8')
        if len(line) > MAX_ENV_LINE_BYTES:
            raise ValueError('OPENATCA_DATA line exceeds its bound')
        current_path_info = os.lstat(env_file)
        current_fd_info = os.fstat(fd)
        if ((current_path_info.st_dev, current_path_info.st_ino) != identity
                or (current_fd_info.st_dev, current_fd_info.st_ino) != identity
                or stat.S_ISLNK(current_path_info.st_mode)):
            raise ValueError('GITHUB_ENV changed before append')
        written = os.write(fd, line)
        if written != len(line):
            raise ValueError('GITHUB_ENV append was incomplete')
        os.fsync(fd)
        _validate_fixture(root, runner_temp, uid)
    except BaseException:
        if root is not None:
            try:
                (root / DATA_ROOT_SENTINEL).unlink(missing_ok=True)
                root.rmdir()
            except OSError:
                pass
        raise
    finally:
        os.close(fd)
    return {
        'schema': 'openatca-ci-test-data-root-receipt-v1',
        'status': 'issued',
        'sentinel': DATA_ROOT_SENTINEL,
        'sentinel_bytes': len(DATA_ROOT_SENTINEL_BYTES),
        'sentinel_sha256': hashlib.sha256(DATA_ROOT_SENTINEL_BYTES).hexdigest(),
    }


def main(environment: dict[str, str] | None = None) -> int:
    environment = os.environ if environment is None else environment
    try:
        receipt = configure_test_data_root(environment)
    except (OSError, ValueError, RuntimeError):
        print(json.dumps({'schema': 'openatca-ci-test-data-root-receipt-v1',
                          'status': 'refused'}))
        return 2
    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
