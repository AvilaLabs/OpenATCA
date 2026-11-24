#!/usr/bin/env python3
"""Bounded, private development capture of adsb.lol regional V2 snapshots.

This tool never scores data. It retains each exact HTTPS response before decoding it, streams
OAF2 MESSAGE/ADS3 frames from the sibling oa-adsb-snapshot executable, and publishes COMPLETE
only after every response, output, and manifest has been flushed and hashed. Do not use these
snapshot-derived timestamp estimates as precision benchmark truth without a separate evaluation.
"""
from __future__ import annotations

import argparse
import calendar
import http.client
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import stat
import sys
import selectors
import signal
import struct
import time
from datetime import datetime, timezone
import re
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, quote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tools"))
import sealed_guard  # noqa: E402

MAX_DURATION_S = 300
MIN_POLL_S = 5
MAX_POLL_S = 300
MAX_RESPONSES = 60
MAX_BODY_BYTES = 1_048_576
MAX_TOTAL_BODY_BYTES = 64 * 1024 * 1024
MAX_TOTAL_OAF_BYTES = 256 * 1024 * 1024
MAX_AGE_MS = 30_000
UA = "OpenATCA-development-capture/1.0 (private research; contact Connor Avila)"
FIXED_QUERY = (42.5, -88.5, 250)
MAX_DECODER_STDOUT = 16 * 1024 * 1024
MAX_DECODER_STDERR = 64 * 1024
MAX_FETCH_HEADERS = 32
MAX_FETCH_HEADER_BYTES = 32 * 1024
MAX_FETCH_WORKER_STDOUT = MAX_BODY_BYTES + MAX_FETCH_HEADER_BYTES + 16
FETCH_MAGIC = b"OAHTTP1\0"
FETCH_WORKER_SWITCH = "--internal-fetch-worker"


def utc_ms() -> int:
    return time.time_ns() // 1_000_000


def utc_text(ms: int | None = None) -> str:
    if ms is None:
        ms = utc_ms()
    seconds, millis = divmod(ms, 1000)
    value = datetime.fromtimestamp(seconds, timezone.utc).replace(microsecond=millis * 1000)
    return value.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def digest(path: Path, guard: sealed_guard.Guard | None = None) -> str:
    guard = guard or sealed_guard.production()
    h = hashlib.sha256()
    with guard.open(path, "rb") as f:
        before = os.fstat(f.fileno())
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
        after = os.fstat(f.fileno())
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            raise RuntimeError(f"file changed while hashing: {path}")
    return h.hexdigest()


def digest_open_file(file) -> str:
    before = os.fstat(file.fileno())
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise RuntimeError("decoder must remain a single-link regular file")
    file.seek(0)
    h = hashlib.sha256()
    for chunk in iter(lambda: file.read(1024 * 1024), b""):
        h.update(chunk)
    after = os.fstat(file.fileno())
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise RuntimeError("decoder changed while hashing")
    file.seek(0)
    return h.hexdigest()


def create_exclusive(path: Path, data: bytes, guard: sealed_guard.Guard | None = None) -> None:
    guard = guard or sealed_guard.production()
    guard.check(str(path))
    fd = -1
    created_inode: tuple[int, int] | None = None
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        opened_stat = os.fstat(fd)
        created_inode = (opened_stat.st_dev, opened_stat.st_ino)
        guard.verify_fd(fd, path)
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1:
            raise RuntimeError(f"new output is not a single-link regular file: {path}")
        path_stat = os.stat(path, follow_symlinks=False)
        if (st.st_dev, st.st_ino) != (path_stat.st_dev, path_stat.st_ino):
            raise RuntimeError(f"new output path changed before write: {path}")
        view = memoryview(data)
        while view:
            written = os.write(fd, view)
            if written <= 0:
                raise OSError("short write")
            view = view[written:]
        os.fsync(fd)
        guard.check(str(path))
        path_stat = os.stat(path, follow_symlinks=False)
        after = os.fstat(fd)
        if (after.st_dev, after.st_ino) != (path_stat.st_dev, path_stat.st_ino):
            raise RuntimeError(f"output path changed while writing: {path}")
    except BaseException:
        if fd >= 0:
            try:
                guard.check(str(path))
                current = os.stat(path, follow_symlinks=False)
                if created_inode == (current.st_dev, current.st_ino):
                    path.unlink(missing_ok=True)
            except (OSError, sealed_guard.SealedError):
                pass
        raise
    finally:
        if fd >= 0:
            os.close(fd)


def check_time(guard: sealed_guard.Guard, millis: int, label: str) -> None:
    if guard.in_window(millis * 1_000_000):
        raise RuntimeError(f"{label} falls in a compiled sealed window")


def check_cohort(guard: sealed_guard.Guard, start: int, end: int) -> None:
    if any(start * 1_000_000 < right and left < end * 1_000_000 for _, left, right in guard.windows):
        raise RuntimeError("preregistered cohort interval intersects a compiled sealed window")


def sync_dir(path: Path, guard: sealed_guard.Guard) -> None:
    guard.check(str(path))
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        guard.verify_fd(fd, path)
        os.fsync(fd)
    finally:
        os.close(fd)


def publish_exclusive(source: Path, destination: Path, guard: sealed_guard.Guard) -> None:
    guard.check(str(source))
    guard.check(str(destination))
    with guard.open(source, "rb") as source_file:
        source_stat = os.fstat(source_file.fileno())
        path_stat = os.stat(source, follow_symlinks=False)
        if not stat.S_ISREG(source_stat.st_mode) or source_stat.st_nlink != 1:
            raise RuntimeError("staged output must be a single-link regular file")
        if (source_stat.st_dev, source_stat.st_ino) != (path_stat.st_dev, path_stat.st_ino):
            raise RuntimeError("staged output path changed before publication")
        guard.check(str(source))
        guard.check(str(destination))
        os.link(source, destination)
        linked_stat = os.stat(destination, follow_symlinks=False)
        if (source_stat.st_dev, source_stat.st_ino) != (linked_stat.st_dev, linked_stat.st_ino):
            guard.check(str(destination))
            destination.unlink()
            raise RuntimeError("published output inode did not match the guarded staging file")
    guard.check(str(source))
    guard.check(str(destination))
    source.unlink()
    guard.check(str(destination))


def check_snapshot_times(body: bytes, receipt_ms: int, guard: sealed_guard.Guard) -> tuple[int, int]:
    check_time(guard, receipt_ms, "local response receipt")
    i = 0

    def ws(pos: int) -> int:
        while pos < len(body) and body[pos] in b" \t\r\n":
            pos += 1
        return pos

    def string_end(pos: int) -> int:
        if pos >= len(body) or body[pos] != 34:
            raise RuntimeError("snapshot envelope has an invalid JSON string")
        pos += 1
        while pos < len(body):
            byte = body[pos]
            if byte == 34:
                return pos + 1
            if byte < 0x20:
                raise RuntimeError("snapshot envelope has an invalid JSON string")
            if byte == 92:
                pos += 2
            else:
                pos += 1
        raise RuntimeError("snapshot envelope has an unterminated JSON string")

    def value_end(pos: int, depth: int) -> tuple[int, bytes | None]:
        if depth > 64:
            raise RuntimeError("snapshot JSON nesting exceeds 64 levels")
        pos = ws(pos)
        if pos >= len(body):
            raise RuntimeError("snapshot JSON value is missing")
        if body[pos] == 34:
            return string_end(pos), None
        if body[pos] == 123:
            pos = ws(pos + 1)
            if pos < len(body) and body[pos] == 125:
                return pos + 1, None
            while True:
                key_end = string_end(pos)
                pos = ws(key_end)
                if pos >= len(body) or body[pos] != 58:
                    raise RuntimeError("snapshot JSON object is malformed")
                pos, _ = value_end(pos + 1, depth + 1)
                pos = ws(pos)
                if pos < len(body) and body[pos] == 125:
                    return pos + 1, None
                if pos >= len(body) or body[pos] != 44:
                    raise RuntimeError("snapshot JSON object is malformed")
                pos = ws(pos + 1)
        if body[pos] == 91:
            pos = ws(pos + 1)
            if pos < len(body) and body[pos] == 93:
                return pos + 1, None
            while True:
                pos, _ = value_end(pos, depth + 1)
                pos = ws(pos)
                if pos < len(body) and body[pos] == 93:
                    return pos + 1, None
                if pos >= len(body) or body[pos] != 44:
                    raise RuntimeError("snapshot JSON array is malformed")
                pos = ws(pos + 1)
        start = pos
        while pos < len(body) and body[pos] not in b" \t\r\n,]}:":
            pos += 1
        if pos == start:
            raise RuntimeError("snapshot JSON scalar is malformed")
        return pos, body[start:pos]

    i = ws(i)
    if i >= len(body) or body[i] != 123:
        raise RuntimeError("snapshot envelope is not an object")
    i = ws(i + 1)
    times: dict[str, int] = {}
    if i < len(body) and body[i] == 125:
        raise RuntimeError("snapshot envelope lacks now/ctime")
    while True:
        key_start = i
        key_end = string_end(i)
        try:
            key = json.loads(body[key_start:key_end].decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise RuntimeError("snapshot envelope has an invalid key") from exc
        i = ws(key_end)
        if i >= len(body) or body[i] != 58:
            raise RuntimeError("snapshot envelope is malformed")
        i = ws(i + 1)
        end, scalar = value_end(i, 1)
        if key in ("now", "ctime"):
            if key in times or scalar is None or not re.fullmatch(rb"[0-9]{1,16}", scalar):
                raise RuntimeError(f"snapshot {key} is not a unique integer Unix-millisecond timestamp")
            value = int(scalar)
            check_time(guard, value, f"snapshot {key}")
            times[key] = value
        i = ws(end)
        if i < len(body) and body[i] == 125:
            i = ws(i + 1)
            break
        if i >= len(body) or body[i] != 44:
            raise RuntimeError("snapshot envelope is malformed")
        i = ws(i + 1)
    if i != len(body) or set(times) != {"now", "ctime"}:
        raise RuntimeError("snapshot envelope lacks unique now/ctime or has trailing data")
    return times["now"], times["ctime"]


def bounded_process(command: list[str], body: bytes, timeout_s: int, stdout_limit: int, stderr_limit: int,
                    pass_fds: tuple[int, ...] = ()) -> tuple[int, bytes, bytes]:
    deadline = time.monotonic() + timeout_s
    proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            close_fds=True, pass_fds=pass_fds, start_new_session=True)
    stdin, stdout, stderr = proc.stdin, proc.stdout, proc.stderr
    if stdin is None or stdout is None or stderr is None:
        proc.kill()
        proc.wait()
        raise RuntimeError("decoder subprocess pipes were not created")
    selector = selectors.DefaultSelector()
    outputs = [bytearray(), bytearray()]
    limits = [stdout_limit, stderr_limit]
    streams = {stdout: 0, stderr: 1}
    pending = memoryview(body)
    cleanup_started = False

    def kill_group() -> None:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError:
            try:
                proc.kill()
            except OSError:
                pass

    def stop_and_reap() -> None:
        nonlocal cleanup_started
        cleanup_started = True
        kill_group()
        try:
            proc.wait(timeout=1.0)
            return
        except subprocess.TimeoutExpired:
            kill_group()
        try:
            proc.wait(timeout=1.0)
        except subprocess.TimeoutExpired as exc:
            raise RuntimeError("decoder/fetch worker did not exit within 2 s after process-group SIGKILL; the OS may be holding it in an uninterruptible state") from exc

    try:
        for stream in (stdin, stdout, stderr):
            os.set_blocking(stream.fileno(), False)
        selector.register(stdout, selectors.EVENT_READ, "stdout")
        selector.register(stderr, selectors.EVENT_READ, "stderr")
        if pending:
            selector.register(stdin, selectors.EVENT_WRITE, "stdin")
        else:
            stdin.close()
        while True:
            remaining = deadline - time.monotonic()
            if proc.poll() is not None and not selector.get_map():
                if remaining < 0:
                    raise subprocess.TimeoutExpired(command, timeout_s)
                break
            if remaining <= 0:
                raise subprocess.TimeoutExpired(command, timeout_s)
            events = selector.select(remaining)
            for key, _ in events:
                stream = key.fileobj
                if key.data == "stdin":
                    try:
                        sent = os.write(stream.fileno(), pending[:65_536])
                    except BlockingIOError:
                        continue
                    except BrokenPipeError:
                        selector.unregister(stream)
                        stream.close()
                        continue
                    if sent <= 0:
                        raise BrokenPipeError("decoder closed its input")
                    pending = pending[sent:]
                    if not pending:
                        selector.unregister(stream)
                        stream.close()
                    continue
                try:
                    chunk = os.read(stream.fileno(), 8192)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(stream)
                    stream.close()
                    continue
                slot = streams[stream]
                remaining_capacity = limits[slot] - len(outputs[slot])
                if len(chunk) > remaining_capacity:
                    if remaining_capacity > 0:
                        outputs[slot].extend(chunk[:remaining_capacity])
                    raise RuntimeError("snapshot decoder exceeded stdout/stderr cap")
                outputs[slot].extend(chunk)
        code = proc.wait()
    except BaseException:
        stop_and_reap()
        raise
    finally:
        try:
            if proc.poll() is None and not cleanup_started:
                stop_and_reap()
        finally:
            selector.close()
            for stream in (stdin, stdout, stderr):
                if not stream.closed:
                    stream.close()
    return code, bytes(outputs[0]), bytes(outputs[1])


def request_url(lat: float, lon: float, radius_nm: int) -> str:
    return ("https://api.adsb.lol/v2/lat/" + quote(f"{lat:.5f}", safe="") + "/lon/"
            + quote(f"{lon:.5f}", safe="") + "/dist/" + str(radius_nm))


def metar_url(stations: str) -> str:
    return "https://aviationweather.gov/api/data/metar?ids=" + quote(stations, safe=",") + "&format=json"


def _validate_fetch_url(url: str) -> None:
    if not isinstance(url, str) or len(url) > 2048:
        raise ValueError("fetch URL must be at most 2048 characters")
    parsed = urlsplit(url)
    if parsed.scheme != "https" or parsed.username is not None or parsed.password is not None or parsed.fragment:
        raise ValueError("fetch URL must be HTTPS without credentials or a fragment")
    try:
        port = parsed.port
    except ValueError as exc:
        raise ValueError("fetch URL has an invalid port") from exc
    if port not in (None, 443):
        raise ValueError("fetch URL must use the standard HTTPS port")
    if parsed.hostname == "api.adsb.lol":
        if parsed.query or re.fullmatch(r"/v2/lat/-?[0-9]+\.[0-9]{5}/lon/-?[0-9]+\.[0-9]{5}/dist/[0-9]{1,3}", parsed.path) is None:
            raise ValueError("adsb.lol fetch URL is outside the bounded regional V2 endpoint")
        return
    if parsed.hostname == "aviationweather.gov" and parsed.path == "/api/data/metar":
        pairs = parse_qs(parsed.query, strict_parsing=True)
        stations = pairs.get("ids", [""])
        if set(pairs) != {"ids", "format"} or pairs.get("format") != ["json"] or len(stations) != 1:
            raise ValueError("METAR fetch URL must contain ids and format=json only")
        ids = stations[0].split(",")
        if not 1 <= len(ids) <= 32 or any(re.fullmatch(r"[A-Z]{4}", station) is None for station in ids):
            raise ValueError("METAR fetch URL must list 1 to 32 uppercase four-letter stations")
        return
    raise ValueError("fetch URL host is not an approved public endpoint")


def fetch_worker_command(url: str, timeout_s: int, allow_204: bool) -> list[str]:
    _validate_fetch_url(url)
    if not 1 <= timeout_s <= 15:
        raise ValueError("fetch timeout must be 1 to 15 seconds")
    return [sys.executable, str(Path(__file__).resolve()), FETCH_WORKER_SWITCH, url, str(timeout_s), "1" if allow_204 else "0"]


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, _request, _fp, _code, _message, _headers, _newurl):
        return None


def _urlopen_no_redirect(request: Request, timeout_s: int):
    return build_opener(_NoRedirect).open(request, timeout=timeout_s)


def _fetch_response(url: str, timeout_s: int, *, allow_204: bool = False) -> tuple[bytes, dict[str, str], int, int, int]:
    # This runs only in a killable worker process. urllib's socket timeout is an inactivity
    # timeout; the parent enforces the total DNS/connect/header/body deadline.
    http.client._MAXHEADERS = MAX_FETCH_HEADERS
    http.client._MAXLINE = 8192
    request = Request(url, headers={"Accept": "application/json", "User-Agent": UA})
    try:
        with _urlopen_no_redirect(request, timeout_s) as response:
            if response.status != 200 and not (allow_204 and response.status == 204):
                raise RuntimeError(f"HTTP status {response.status}")
            body = response.read(MAX_BODY_BYTES + 1)
            all_headers = list(response.headers.raw_items())
            if len(all_headers) > MAX_FETCH_HEADERS:
                raise RuntimeError("response has too many HTTP headers")
            encoded_headers = [(str(key), str(value)) for key, value in all_headers]
            if sum(len(k.encode("utf-8")) + len(v.encode("utf-8")) + 4 for k, v in encoded_headers) > MAX_FETCH_HEADER_BYTES:
                raise RuntimeError("response HTTP headers exceed the 32 KiB limit")
            headers = {key: response.headers.get(key, "") for key in ("Date", "ETag", "Last-Modified", "Content-Type")}
            status = response.status
    except HTTPError as e:
        retry_after = e.headers.get("Retry-After", "")[:128]
        raise RuntimeError(f"HTTP status {e.code}; Retry-After={retry_after}") from e
    except URLError as e:
        raise RuntimeError(f"public data request failed: {type(e.reason).__name__}") from e
    received_mono = time.monotonic_ns()
    received_ms = utc_ms()
    if len(body) > MAX_BODY_BYTES:
        raise RuntimeError(f"response exceeds {MAX_BODY_BYTES} bytes")
    return body, headers, status, received_ms, received_mono


def _fetch_worker_main(argv: list[str]) -> int:
    if len(argv) != 4:
        print("fetch worker: invalid argument count", file=sys.stderr)
        return 2
    _, url, timeout_text, allow_text = argv
    try:
        timeout_s = int(timeout_text)
        if allow_text not in ("0", "1"):
            raise ValueError("invalid allow-204 flag")
        if not 1 <= timeout_s <= 15:
            raise ValueError("worker timeout must be 1 to 15 seconds")
        _validate_fetch_url(url)
        body, headers, status, received_ms, received_mono = _fetch_response(
            url, timeout_s, allow_204=(allow_text == "1"))
        meta = json.dumps({"headers": headers, "status": status, "received_ms": received_ms,
                           "received_mono_ns": received_mono}, separators=(",", ":"), sort_keys=True).encode("utf-8")
        if len(meta) > MAX_FETCH_HEADER_BYTES:
            raise RuntimeError("response metadata exceeds the 32 KiB limit")
        packet = FETCH_MAGIC + struct.pack("<II", len(meta), len(body)) + meta + body
        sys.stdout.buffer.write(packet)
        sys.stdout.buffer.flush()
        return 0
    except (OSError, ValueError, RuntimeError, HTTPError, URLError) as exc:
        message = str(exc).replace("\n", " ")[:512]
        print(f"fetch worker: {message}", file=sys.stderr)
        return 2


def run_fetch_worker(command: list[str], timeout_s: int) -> tuple[bytes, dict[str, str], int, int, int]:
    code, packet, stderr = bounded_process(command, b"", timeout_s, MAX_FETCH_WORKER_STDOUT, MAX_DECODER_STDERR)
    if code != 0:
        detail = stderr.decode("utf-8", "replace")[-512:]
        raise RuntimeError(f"public data request failed: {detail or f'worker exit {code}'}")
    if len(packet) < len(FETCH_MAGIC) + 8 or not packet.startswith(FETCH_MAGIC):
        raise RuntimeError("fetch worker returned a malformed packet")
    meta_len, body_len = struct.unpack_from("<II", packet, len(FETCH_MAGIC))
    start = len(FETCH_MAGIC) + 8
    if meta_len > MAX_FETCH_HEADER_BYTES or body_len > MAX_BODY_BYTES or len(packet) != start + meta_len + body_len:
        raise RuntimeError("fetch worker packet exceeds its declared limits")
    try:
        metadata = json.loads(packet[start:start + meta_len].decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError, RecursionError) as exc:
        raise RuntimeError("fetch worker returned invalid response metadata") from exc
    if not isinstance(metadata, dict):
        raise RuntimeError("fetch worker response metadata is not an object")
    headers, status = metadata.get("headers"), metadata.get("status")
    received_ms, received_mono = metadata.get("received_ms"), metadata.get("received_mono_ns")
    if (not isinstance(headers, dict) or len(headers) > MAX_FETCH_HEADERS
            or any(not isinstance(k, str) or not isinstance(v, str) for k, v in headers.items())
            or isinstance(status, bool) or not isinstance(status, int)
            or isinstance(received_ms, bool) or not isinstance(received_ms, int)
            or not 100 <= status <= 599
            or not 946_684_800_000 <= received_ms < 4_102_444_800_000
            or isinstance(received_mono, bool) or not isinstance(received_mono, int)
            or not 0 <= received_mono < 2**64):
        raise RuntimeError("fetch worker response metadata has invalid fields")
    body = packet[start + meta_len:]
    return body, headers, status, received_ms, received_mono


def fetch_once(url: str, timeout_s: int, *, allow_204: bool = False) -> tuple[bytes, dict[str, str], int, int, int]:
    command = fetch_worker_command(url, timeout_s, allow_204)
    return run_fetch_worker(command, timeout_s)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--out-dir", required=True, type=Path)
    p.add_argument("--decoder", required=True, type=Path, help="prebuilt oa-adsb-snapshot executable")
    p.add_argument("--cohort-from-utc", required=True)
    p.add_argument("--cohort-to-utc", required=True)
    p.add_argument("--lat", type=float, default=42.5)
    p.add_argument("--lon", type=float, default=-88.5)
    p.add_argument("--radius-nm", type=int, default=250)
    p.add_argument("--duration-s", type=int, default=300)
    p.add_argument("--poll-s", type=int, default=5)
    p.add_argument("--timeout-s", type=int, default=10)
    p.add_argument("--max-age-ms", type=int, default=15_000)
    p.add_argument("--metar-stations", default="KORD,KMDW,KBMI,KIND,KMSN")
    return p.parse_args()


def parse_utc(text: str) -> int:
    match = re.fullmatch(r"([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]{1,3}))?(Z|[+-][0-9]{2}:?[0-9]{2})", text)
    if match is None:
        raise ValueError("UTC bounds need an exact millisecond timestamp and explicit Z or offset")
    year, month, day, hour, minute, second = (int(x) for x in match.groups()[:6])
    fraction = (match.group(7) or "").ljust(3, "0")
    zone = match.group(8)
    try:
        epoch_seconds = calendar.timegm((year, month, day, hour, minute, second))
        datetime(year, month, day, hour, minute, second)
    except (OverflowError, ValueError) as exc:
        raise ValueError("invalid UTC calendar timestamp") from exc
    if zone != "Z":
        sign = 1 if zone[0] == "+" else -1
        digits = zone[1:].replace(":", "")
        offset_h, offset_m = int(digits[:2]), int(digits[2:])
        if offset_h > 23 or offset_m > 59:
            raise ValueError("invalid UTC offset")
        epoch_seconds -= sign * (offset_h * 3600 + offset_m * 60)
    return epoch_seconds * 1000 + (int(fraction) if fraction else 0)


def checked_options(a: argparse.Namespace) -> tuple[int, int]:
    for name in ("duration_s", "poll_s", "timeout_s", "max_age_ms", "radius_nm"):
        value = getattr(a, name)
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValueError(f"{name} must be an integer")
    for name in ("lat", "lon"):
        value = getattr(a, name)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise ValueError(f"{name} must be a finite number")
    if not isinstance(a.metar_stations, str):
        raise ValueError("METAR stations must be text")
    if not 1 <= a.duration_s <= MAX_DURATION_S:
        raise ValueError(f"duration must be 1 to {MAX_DURATION_S} seconds")
    if not MIN_POLL_S <= a.poll_s <= MAX_POLL_S:
        raise ValueError(f"poll interval must be {MIN_POLL_S} to {MAX_POLL_S} seconds")
    if not 1 <= a.timeout_s <= 15:
        raise ValueError("HTTP timeout must be 1 to 15 seconds")
    if not 1 <= a.max_age_ms <= MAX_AGE_MS:
        raise ValueError(f"max age must be 1 to {MAX_AGE_MS} milliseconds")
    if not -90 <= a.lat <= 90 or not -180 <= a.lon <= 180 or not 1 <= a.radius_nm <= 250:
        raise ValueError("invalid query center/radius; API radius is 1 to 250 nm")
    if (a.lat, a.lon, a.radius_nm) != FIXED_QUERY:
        raise ValueError("decoder applies the fixed ZAU box; query must use the declared ZAU coverage circle")
    start, end = parse_utc(a.cohort_from_utc), parse_utc(a.cohort_to_utc)
    if not start < end:
        raise ValueError("cohort from bound must be before to bound")
    if a.duration_s * 1000 > end - start:
        raise ValueError("capture duration exceeds the preregistered cohort interval")
    stations = a.metar_stations.split(",")
    if not 1 <= len(stations) <= 32 or any(len(s) != 4 or not s.isascii() or not s.isalpha() or s.upper() != s for s in stations):
        raise ValueError("METAR stations must be comma-separated uppercase four-letter identifiers")
    if len(metar_url(a.metar_stations)) > 2048:
        raise ValueError("METAR URL exceeds the 2048-character limit")
    return start, end


def main() -> int:
    a = parse_args()
    guard = sealed_guard.production()
    reserved_out: Path | None = None
    entries: list[dict[str, object]] = []
    raw_files: list[dict[str, object]] = []
    total_body = total_oaf = requests = frame_seq = 0
    oaf_digest = hashlib.sha256()
    complete_written = False
    decoder_file = None
    try:
        cohort_from, cohort_to = checked_options(a)
        check_cohort(guard, cohort_from, cohort_to)
        check_time(guard, utc_ms(), "capture planning time")
        guard.check(str(a.out_dir.parent))
        guard.check(str(a.out_dir))
        guard.check(str(a.decoder))
        private_path = a.out_dir.resolve()
        if "openatca-private" not in private_path.parts:
            raise RuntimeError("capture outputs must be stored below an openatca-private directory")
        if a.out_dir.exists() or a.out_dir.is_symlink():
            raise RuntimeError("output directory exists; refusing overwrite")
        if not a.decoder.is_file() or not os.access(a.decoder, os.X_OK):
            raise RuntimeError("decoder is not an executable file")
        decoder_file = guard.open(a.decoder, "rb")
        decoder_path_record = a.decoder.resolve()
        guard.check(str(decoder_path_record))
        decoder_stat = os.fstat(decoder_file.fileno())
        if not stat.S_ISREG(decoder_stat.st_mode) or decoder_stat.st_nlink != 1:
            raise RuntimeError("decoder must be a single-link regular file")
        if decoder_stat.st_size <= 0:
            raise RuntimeError("decoder executable is empty")
        decoder_hash = digest_open_file(decoder_file)
        a.out_dir.mkdir(mode=0o700)
        reserved_out = a.out_dir
        guard.check(str(reserved_out))
        start_wall, start_mono = utc_ms(), time.monotonic_ns()
        check_time(guard, start_wall, "capture start")
        if start_wall < cohort_from:
            raise RuntimeError("capture start precedes the preregistered cohort interval")
        if start_wall >= cohort_to:
            raise RuntimeError("capture start is at or beyond the preregistered cohort end")
        responses = a.out_dir / "responses"
        guard.check(str(responses))
        responses.mkdir(mode=0o700)
        guard.check(str(responses))
        part = a.out_dir / "aircraft.oaf.part"
        guard.check(str(part))
        stop_at = start_mono + a.duration_s * 1_000_000_000
        check_time(guard, utc_ms(), "METAR request time")
        metar_body, metar_headers, metar_status, metar_ms, metar_mono = fetch_once(metar_url(a.metar_stations), a.timeout_s, allow_204=True)
        check_time(guard, metar_ms, "METAR response receipt")
        if not cohort_from <= metar_ms < cohort_to:
            raise RuntimeError("METAR response receipt falls outside the preregistered cohort interval")
        metar_path = responses / "metar-current.json"
        create_exclusive(metar_path, metar_body, guard)
        metar_record = {"source": "official Aviation Weather Center Data API", "url": metar_url(a.metar_stations),
                        "stations": a.metar_stations, "http_status": metar_status,
                        "received_utc": utc_text(metar_ms), "received_unix_ms": metar_ms,
                        "received_monotonic_ns": metar_mono, "headers": metar_headers,
                        "raw_file": "responses/metar-current.json", "raw_bytes": len(metar_body),
                        "raw_sha256": hashlib.sha256(metar_body).hexdigest()}
        total_body += len(metar_body)
        raw_files.append({"file": "responses/metar-current.json", "bytes": len(metar_body), "sha256": hashlib.sha256(metar_body).hexdigest()})
        if total_body > MAX_TOTAL_BODY_BYTES:
            raise RuntimeError(f"cumulative response bytes exceed {MAX_TOTAL_BODY_BYTES}")
        url = request_url(a.lat, a.lon, a.radius_nm)
        part_fd = os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        try:
            guard.verify_fd(part_fd, part)
            part_stat = os.fstat(part_fd)
            path_stat = os.stat(part, follow_symlinks=False)
            if not stat.S_ISREG(part_stat.st_mode) or part_stat.st_nlink != 1 or (part_stat.st_dev, part_stat.st_ino) != (path_stat.st_dev, path_stat.st_ino):
                raise RuntimeError("OAF staging output is not a regular file")
            with os.fdopen(part_fd, "wb") as clip:
                while time.monotonic_ns() < stop_at and requests < MAX_RESPONSES:
                    if utc_ms() >= cohort_to:
                        raise RuntimeError("capture reached the preregistered cohort end")
                    check_time(guard, utc_ms(), "snapshot request time")
                    body, headers, status, receipt_ms, receipt_mono = fetch_once(url, a.timeout_s)
                    if not cohort_from <= receipt_ms < cohort_to:
                        raise RuntimeError("snapshot response receipt falls outside the preregistered cohort interval")
                    source_now_ms, source_ctime_ms = check_snapshot_times(body, receipt_ms, guard)
                    requests += 1
                    total_body += len(body)
                    if total_body > MAX_TOTAL_BODY_BYTES:
                        raise RuntimeError(f"cumulative response bytes exceed {MAX_TOTAL_BODY_BYTES}")
                    name = f"snapshot-{requests:03}.json"
                    raw = responses / name
                    create_exclusive(raw, body, guard)
                    body_hash = hashlib.sha256(body).hexdigest()
                    raw_files.append({"file": f"responses/{name}", "bytes": len(body), "sha256": body_hash})
                    entry = {"index": requests, "raw_file": f"responses/{name}", "http_status": status,
                             "received_utc": utc_text(receipt_ms), "received_unix_ms": receipt_ms,
                             "received_monotonic_ns": receipt_mono, "raw_bytes": len(body),
                             "raw_sha256": body_hash, "response_headers": headers,
                             "source_now_unix_ms": source_now_ms, "source_ctime_unix_ms": source_ctime_ms,
                             "max_age_ms": a.max_age_ms}
                    entries.append(entry)
                    seq = frame_seq
                    current_decoder_hash = digest_open_file(decoder_file)
                    if current_decoder_hash != decoder_hash:
                        raise RuntimeError("decoder executable hash changed during capture")
                    execute_path = f"/proc/self/fd/{decoder_file.fileno()}"
                    command = [execute_path, "--received-unix-ms", str(receipt_ms), "--received-mono-ns", str(receipt_mono), "--seq", str(seq), "--max-age-ms", str(a.max_age_ms)]
                    entry["decoder_argv"] = command
                    returncode, stdout, stderr = bounded_process(command, body, 15, MAX_DECODER_STDOUT, MAX_DECODER_STDERR,
                                                                 (decoder_file.fileno(),))
                    entry["decoder_stderr"] = stderr.decode("utf-8", "replace").strip()
                    if returncode != 0:
                        raise RuntimeError(f"snapshot decoder failed for {name}: {stderr.decode('utf-8', 'replace')[-500:]}")
                    if total_oaf + len(stdout) > MAX_TOTAL_OAF_BYTES:
                        raise RuntimeError(f"cumulative OAF bytes exceed {MAX_TOTAL_OAF_BYTES}")
                    view = memoryview(stdout)
                    while view:
                        written = clip.write(view)
                        if written is None or written <= 0:
                            raise OSError("short OAF write")
                        view = view[written:]
                    oaf_digest.update(stdout)
                    clip.flush()
                    os.fsync(clip.fileno())
                    total_oaf += len(stdout)
                    if stdout:
                        frame_seq += 1
                    entry["oaf_frame_bytes"] = len(stdout)
                    wait_ns = a.poll_s * 1_000_000_000 - (time.monotonic_ns() - receipt_mono)
                    if wait_ns > 0 and time.monotonic_ns() + wait_ns < stop_at:
                        time.sleep(wait_ns / 1_000_000_000)
                clip.flush()
                os.fsync(clip.fileno())
        except BaseException:
            try:
                os.close(part_fd)
            except OSError:
                pass
            raise
        stop_wall, stop_mono = utc_ms(), time.monotonic_ns()
        check_time(guard, stop_wall, "capture stop")
        if requests == 0:
            raise RuntimeError("no snapshots were captured")
        if stop_wall >= cohort_to:
            raise RuntimeError("capture stop exceeded the preregistered cohort end")
        final_clip = a.out_dir / "aircraft.oaf"
        publish_exclusive(part, final_clip, guard)
        tool_path = Path(__file__).resolve()
        guard.check(str(tool_path))
        manifest = {
            "schema": "openatca.adsb-live-capture.v1", "status": "complete",
            "source": "adsb.lol V2 regional JSON snapshot API; ODbL 1.0; each raw response retained unchanged",
            "endpoint": url, "request_user_agent": UA,
            "query": {"lat": a.lat, "lon": a.lon, "radius_nm": a.radius_nm,
                      "decoder_geographic_filter": "fixed ZAU box: 38.5–46.5 N, 83.5–93.5 W; outside rows excluded"},
            "capture_start_utc": utc_text(start_wall), "capture_stop_utc": utc_text(stop_wall),
            "capture_start_monotonic_ns": start_mono, "capture_stop_monotonic_ns": stop_mono,
            "duration_limit_s": a.duration_s, "poll_interval_s": a.poll_s, "http_timeout_s": a.timeout_s,
            "cohort_interval_utc": {"from_inclusive": a.cohort_from_utc, "to_exclusive": a.cohort_to_utc},
            "cohort_timestamp_policy": "ADS-B estimated report time is server now minus raw seen_pos; exact source response preserves age values; timestamp age/quantization uncertainty remains; not precision truth by default",
            "decoder_path": str(decoder_path_record), "decoder_sha256": decoder_hash,
            "capture_tool_path": str(tool_path), "capture_tool_sha256": digest(tool_path, guard),
            "decoder_source": "oa-adsb-snapshot; existing ADS3 payload carried in OAF2 MESSAGE frames with local receipt wall and monotonic times",
            "caps": {"max_responses": MAX_RESPONSES, "max_body_bytes": MAX_BODY_BYTES, "max_total_body_bytes": MAX_TOTAL_BODY_BYTES,
                     "max_total_oaf_bytes": MAX_TOTAL_OAF_BYTES, "max_position_age_ms": MAX_AGE_MS},
            "totals": {"responses": requests, "body_bytes": total_body, "oaf_bytes": total_oaf},
            "snapshots": entries,
            "metar": metar_record,
            "output": {"file": "aircraft.oaf", "bytes": os.stat(final_clip, follow_symlinks=False).st_size,
                       "sha256": digest(final_clip, guard)},
            "terms": "Private development data only. Do not publish FAA-origin SWIM data or claim independent radar truth from ADS-B source flags.",
        }
        manifest_path = a.out_dir / "manifest.json"
        create_exclusive(manifest_path, (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode(), guard)
        marker = ("openatca.adsb-live-capture.complete.v1\nmanifest_sha256=" + digest(manifest_path, guard) + "\n").encode()
        create_exclusive(a.out_dir / "COMPLETE", marker, guard)
        complete_written = True
        sync_dir(responses, guard)
        sync_dir(a.out_dir, guard)
        dir_fd = os.open(a.out_dir, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
        return 0
    except BaseException as e:
        if decoder_file is not None:
            decoder_file.close()
        if reserved_out is not None:
            try:
                guard.check(str(reserved_out))
                if complete_written:
                    complete_path = reserved_out / "COMPLETE"
                    guard.check(str(complete_path))
                    complete_path.unlink()
                marker = reserved_out / "INCOMPLETE.json"
                create_exclusive(marker, (json.dumps({"status": "incomplete", "reason": str(e), "recorded_utc": utc_text(),
                                                      "responses": requests, "response_files": raw_files,
                                                      "snapshots": entries, "body_bytes": total_body,
                                                      "oaf_bytes": total_oaf, "oaf_partial_sha256": oaf_digest.hexdigest()},
                                                     sort_keys=True) + "\n").encode(), guard)
                sync_dir(reserved_out, guard)
            except (OSError, RuntimeError, sealed_guard.SealedError):
                pass
        print(f"capture_adsb_lol: {e}", file=sys.stderr)
        return 2
    finally:
        if decoder_file is not None and not decoder_file.closed:
            decoder_file.close()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == FETCH_WORKER_SWITCH:
        raise SystemExit(_fetch_worker_main(sys.argv[1:]))
    raise SystemExit(main())
