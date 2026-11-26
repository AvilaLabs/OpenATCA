"""Synthetic-only tests for the bounded ADS-B capture wrapper; no network or FAA data."""

import importlib.util
from argparse import Namespace
from pathlib import Path
import tempfile
import unittest
import warnings
from unittest.mock import patch


SCRIPT = Path(__file__).parent / "oa-stdds-ref" / "capture_adsb_lol.py"
SPEC = importlib.util.spec_from_file_location("capture_adsb_lol", SCRIPT)
capture = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(capture)


def args(**override):
    values = {
        "duration_s": 300,
        "poll_s": 5,
        "timeout_s": 10,
        "max_age_ms": 15_000,
        "lat": 42.5,
        "lon": -88.5,
        "radius_nm": 250,
        "cohort_from_utc": "2026-11-24T22:00:00Z",
        "cohort_to_utc": "2026-11-24T22:05:00Z",
        "metar_stations": "KORD,KMDW",
    }
    values.update(override)
    return Namespace(**values)


class CaptureTests(unittest.TestCase):
    def test_endpoint_is_regional_v2_and_radius_is_explicit(self):
        self.assertEqual(capture.request_url(42.5, -88.5, 250),
                         "https://api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250")
        self.assertEqual(capture.metar_url("KORD,KMDW"),
                         "https://aviationweather.gov/api/data/metar?ids=KORD,KMDW&format=json")

    def test_cohort_bounds_are_strict_and_duration_is_bounded(self):
        self.assertEqual(capture.checked_options(args()), (1795557600000, 1795557900000))
        with self.assertRaisesRegex(ValueError, "before"):
            capture.checked_options(args(cohort_to_utc="2026-11-24T22:00:00Z"))

    def test_cohort_times_keep_exact_millisecond_precision(self):
        self.assertEqual(capture.parse_utc("2026-11-24T22:00:00.125Z"), 1_795_557_600_125)
        self.assertEqual(capture.parse_utc("2026-11-25T00:00:00.125+02:00"), 1_795_557_600_125)
        with self.assertRaisesRegex(ValueError, "millisecond"):
            capture.parse_utc("2026-11-24T22:00:00.0001Z")

    def test_station_count_and_decoder_fixed_query_are_bounded(self):
        with self.assertRaisesRegex(ValueError, "four-letter"):
            capture.checked_options(args(metar_stations=",".join(["KORD"] * 33)))
        with self.assertRaisesRegex(ValueError, "fixed ZAU"):
            capture.checked_options(args(lat=40.0))

    def test_fetch_worker_accepts_only_fixed_https_source_families(self):
        with self.assertRaisesRegex(ValueError, "approved public endpoint"):
            capture.fetch_worker_command("https://synthetic.invalid/data", 5, False)
        with self.assertRaisesRegex(ValueError, "credentials"):
            capture.fetch_worker_command("https://user:pass@api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250", 5, False)

    def test_numeric_options_reject_bool_and_float_values(self):
        for override in ({"duration_s": True}, {"poll_s": 5.0}, {"timeout_s": False},
                         {"max_age_ms": 1.5}, {"radius_nm": 250.0}):
            with self.subTest(override=override), self.assertRaisesRegex(ValueError, "integer"):
                capture.checked_options(args(**override))
        with self.assertRaisesRegex(ValueError, "finite number"):
            capture.checked_options(args(lat=float("nan")))

    def test_guarded_exclusive_writer_and_hardlink_publication(self):
        guard = capture.sealed_guard.Guard(
            text="root /tmp\nseal forbidden\nwindow 2026-11-20T00:00:00Z 2026-11-21T00:00:00Z\n")
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "capture.part"
            destination = Path(tmp) / "capture.bin"
            capture.create_exclusive(source, b"synthetic", guard)
            capture.publish_exclusive(source, destination, guard)
            self.assertFalse(source.exists())
            with guard.open(destination, "rb") as stream:
                self.assertEqual(stream.read(), b"synthetic")
            with self.assertRaises(FileExistsError):
                capture.create_exclusive(destination, b"overwrite attempt", guard)
            self.assertEqual(destination.read_bytes(), b"synthetic")

    def test_guard_refuses_sealed_source_cohort_and_envelope_times(self):
        guard = capture.sealed_guard.Guard(
            text="root /tmp\nseal forbidden\nwindow 2026-11-24T22:00:00Z 2026-11-24T22:05:00Z\n")
        with tempfile.TemporaryDirectory() as tmp:
            sealed = Path(tmp) / "forbidden" / "input.bin"
            with self.assertRaises(capture.sealed_guard.SealedError):
                capture.create_exclusive(sealed, b"synthetic", guard)
        with self.assertRaisesRegex(RuntimeError, "cohort interval"):
            capture.check_cohort(guard, 1_795_557_600_000, 1_795_557_900_000)
        body = b'{"ac":[],"ctime":1795557600000,"msg":"No error","now":1795557600000,"ptime":1,"total":0}'
        with self.assertRaisesRegex(RuntimeError, "sealed window"):
            capture.check_snapshot_times(body, 1_795_558_860_000, guard)

    def test_snapshot_time_scan_checks_envelope_before_decoding_rows(self):
        guard = capture.sealed_guard.Guard(
            text="root /tmp\nseal forbidden\nwindow 2026-11-24T22:00:00Z 2026-11-24T22:05:00Z\n")
        body = b'{"ac":[{"text":"brace } ] { content"}],"ctime":1795558860000,"now":1795557600000}'
        with self.assertRaisesRegex(RuntimeError, "snapshot now"):
            capture.check_snapshot_times(body, 1_795_558_860_000, guard)

    def test_decoder_output_is_stopped_at_declared_caps(self):
        command = [capture.sys.executable, "-c", "import sys; sys.stdout.write('x' * 4096)"]
        with self.assertRaisesRegex(RuntimeError, "stdout/stderr cap"):
            capture.bounded_process(command, b"", 5, 128, 128)

    def test_selector_initialization_failure_does_not_spawn_worker(self):
        with patch.object(capture.selectors, "DefaultSelector", side_effect=OSError("selector unavailable")), \
             patch.object(capture.subprocess, "Popen") as popen:
            with self.assertRaisesRegex(OSError, "selector unavailable"):
                capture.bounded_process(["unused"], b"", 1, 10, 10)
        popen.assert_not_called()

    def test_worker_initialization_failure_closes_selector(self):
        class FakeSelector:
            closed = False

            def close(self):
                self.closed = True

        selector = FakeSelector()
        with patch.object(capture.selectors, "DefaultSelector", return_value=selector), \
             patch.object(capture.subprocess, "Popen", side_effect=OSError("spawn failed")):
            with self.assertRaisesRegex(OSError, "spawn failed"):
                capture.bounded_process(["unused"], b"", 1, 10, 10)
        self.assertTrue(selector.closed)

    def test_missing_worker_pipe_uses_bounded_reap_and_closes_selector(self):
        class FakeSelector:
            closed = False

            def close(self):
                self.closed = True

        class FakeProcess:
            pid = 12345
            stdin = None
            stdout = None
            stderr = None
            status = None

            def poll(self):
                return self.status

            def wait(self, *, timeout):
                self.assert_bounded_timeout(timeout)
                self.status = -capture.signal.SIGKILL
                return self.status

            @staticmethod
            def assert_bounded_timeout(timeout):
                if timeout != 1.0:
                    raise AssertionError(f"unexpected wait timeout: {timeout}")

            def kill(self):
                self.status = -capture.signal.SIGKILL

        selector = FakeSelector()
        process = FakeProcess()
        with patch.object(capture.selectors, "DefaultSelector", return_value=selector), \
             patch.object(capture.subprocess, "Popen", return_value=process), \
             patch.object(capture.os, "killpg"):
            with self.assertRaisesRegex(RuntimeError, "pipes were not created"):
                capture.bounded_process(["unused"], b"", 1, 10, 10)
        self.assertEqual(process.status, -capture.signal.SIGKILL)
        self.assertTrue(selector.closed)

    def test_execution_uses_the_open_binary_descriptor(self):
        with open(capture.sys.executable, "rb") as binary:
            command = [f"/proc/self/fd/{binary.fileno()}", "-c", "print('descriptor-ok')"]
            code, stdout, stderr = capture.bounded_process(command, b"", 5, 4096, 4096, (binary.fileno(),))
        self.assertEqual(code, 0, stderr.decode("utf-8", "replace"))
        self.assertEqual(stdout, b"descriptor-ok\n")

    def test_process_pipes_close_without_resource_warnings(self):
        real_popen = capture.subprocess.Popen
        children = []

        def track_child(*command, **options):
            child = real_popen(*command, **options)
            children.append(child)
            return child

        with warnings.catch_warnings():
            warnings.simplefilter("error", ResourceWarning)
            with patch.object(capture.subprocess, "Popen", side_effect=track_child):
                code, stdout, _ = capture.bounded_process(
                    [capture.sys.executable, "-c", "print('closed')"], b"", 5, 4096, 4096)
        self.assertEqual(code, 0)
        self.assertEqual(stdout, b"closed\n")
        self.assertEqual(len(children), 1)
        self.assertTrue(children[0].stdin.closed)
        self.assertTrue(children[0].stdout.closed)
        self.assertTrue(children[0].stderr.closed)

    def test_empty_selector_waits_for_child_within_original_deadline(self):
        class Clock:
            now = 10.0

            def monotonic(self):
                return self.now

        class Pipe:
            def __init__(self, fd):
                self.fd = fd
                self.closed = False

            def fileno(self):
                return self.fd

            def close(self):
                self.closed = True

        class FakeSelector:
            def __init__(self):
                self.keys = {}
                self.closed = False

            def register(self, fileobj, events, data=None):
                key = capture.selectors.SelectorKey(fileobj, fileobj.fileno(), events, data)
                self.keys[fileobj] = key

            def unregister(self, fileobj):
                return self.keys.pop(fileobj)

            def get_map(self):
                return self.keys

            def select(self, timeout=None):
                if not self.keys:
                    clock.now += timeout
                    return []
                return [(key, capture.selectors.EVENT_READ) for key in self.keys.values()]

            def close(self):
                self.closed = True

        class FakeProcess:
            pid = 12345

            def __init__(self):
                self.stdin = Pipe(10)
                self.stdout = Pipe(11)
                self.stderr = Pipe(12)
                self.returncode = None
                self.wait_timeouts = []

            def poll(self):
                if self.returncode is None:
                    clock.now += 0.01
                return self.returncode

            def wait(self, timeout=None):
                self.wait_timeouts.append(timeout)
                if timeout is None:
                    self.returncode = 0
                else:
                    self.assert_wait_is_bounded(timeout)
                    # The child exits shortly after both pipes reach EOF, before the deadline.
                    clock.now += 0.1
                    self.returncode = 0
                return self.returncode

            @staticmethod
            def assert_wait_is_bounded(timeout):
                if not 0 < timeout < 1.0:
                    raise AssertionError(f"wait must use the remaining original deadline: {timeout}")

            def kill(self):
                self.returncode = -capture.signal.SIGKILL

        clock = Clock()
        selector = FakeSelector()
        process = FakeProcess()
        with patch.object(capture.selectors, "DefaultSelector", return_value=selector), \
             patch.object(capture.subprocess, "Popen", return_value=process), \
             patch.object(capture.os, "set_blocking"), \
             patch.object(capture.os, "read", return_value=b""), \
             patch.object(capture.os, "killpg"), \
             patch.object(capture.time, "monotonic", side_effect=clock.monotonic):
            code, stdout, stderr = capture.bounded_process(["unused"], b"", 1, 10, 10)
        self.assertEqual(code, 0)
        self.assertEqual((stdout, stderr), (b"", b""))
        self.assertAlmostEqual(process.wait_timeouts[0], 0.98)
        self.assertIsNone(process.wait_timeouts[1])
        self.assertTrue(selector.closed)

    def test_timeout_kills_and_reaps_child_and_closes_pipes(self):
        real_popen = capture.subprocess.Popen
        children = []

        def track_child(*command, **options):
            child = real_popen(*command, **options)
            children.append(child)
            return child

        with patch.object(capture.subprocess, "Popen", side_effect=track_child):
            started = capture.time.monotonic()
            with self.assertRaises(capture.subprocess.TimeoutExpired):
                capture.bounded_process(
                    [capture.sys.executable, "-c", "import time; time.sleep(10)"],
                    b"x" * (1024 * 1024), 0.5, 4096, 4096)
            elapsed = capture.time.monotonic() - started
        self.assertEqual(len(children), 1)
        self.assertLess(elapsed, 3.0)
        child = children[0]
        self.assertIsNotNone(child.poll())
        self.assertTrue(child.stdin.closed)
        self.assertTrue(child.stdout.closed)
        self.assertTrue(child.stderr.closed)

    def test_timeout_kills_descendant_holding_capture_pipes(self):
        child = "import time; time.sleep(10)"
        parent = "import subprocess,sys; subprocess.Popen([sys.executable,'-c',sys.argv[1]]); print('parent-exit',flush=True)"
        started = capture.time.monotonic()
        with self.assertRaises(capture.subprocess.TimeoutExpired):
            capture.bounded_process(
                [capture.sys.executable, "-c", parent, child], b"", 0.5, 4096, 4096)
        self.assertLess(capture.time.monotonic() - started, 3.0)

    def test_open_binary_digest_detects_in_place_change(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "decoder"
            path.write_bytes(b"first")
            with path.open("rb") as binary:
                first = capture.digest_open_file(binary)
                path.write_bytes(b"second")
                second = capture.digest_open_file(binary)
            self.assertNotEqual(first, second)

    def test_response_age_rate_and_body_limits_are_enforced(self):
        for override, expected in [
            ({"poll_s": 4}, "poll interval"),
            ({"duration_s": 301}, "duration"),
            ({"max_age_ms": 30_001}, "max age"),
            ({"radius_nm": 251}, "radius"),
            ({"cohort_to_utc": "2026-11-24T22:04:00Z"}, "duration"),
        ]:
            with self.subTest(override=override), self.assertRaisesRegex(ValueError, expected):
                capture.checked_options(args(**override))

    def test_worker_fetch_receipt_is_sampled_after_body_and_headers_are_saved(self):
        class FakeHeaders(dict):
            def raw_items(self):
                return self.items()

        class FakeResponse:
            status = 200
            headers = FakeHeaders({"Date": "Tue, 24 Nov 2026 22:00:00 GMT", "ETag": '"synthetic"', "Content-Type": "application/json"})

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self, cap):
                if cap != capture.MAX_BODY_BYTES + 1:
                    raise AssertionError("unexpected read cap")
                return b'{"ac":[],"ctime":1795557600000,"msg":"No error","now":1795557600000,"ptime":1,"total":0}'

        with patch.object(capture, "_urlopen_no_redirect", lambda _request, timeout: FakeResponse()), \
             patch.object(capture.time, "monotonic_ns", return_value=90), \
             patch.object(capture, "utc_ms", return_value=1_795_557_600_100):
            body, headers, status, received_ms, mono = capture._fetch_response(
                "https://api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250", 5)
        self.assertTrue(body.startswith(b'{"ac"'))
        self.assertEqual(headers["ETag"], '"synthetic"')
        self.assertEqual(status, 200)
        self.assertEqual((received_ms, mono), (1_795_557_600_100, 90))

    def test_worker_oversized_response_is_refused_without_network(self):
        class FakeHeaders(dict):
            def raw_items(self):
                return self.items()

        class FakeResponse:
            status = 200
            headers = FakeHeaders()

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self, _cap):
                return b"x" * (capture.MAX_BODY_BYTES + 1)

        with patch.object(capture, "_urlopen_no_redirect", lambda _request, timeout: FakeResponse()):
            with self.assertRaisesRegex(RuntimeError, "exceeds"):
                capture._fetch_response("https://api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250", 5)

    def test_worker_rejects_excessive_response_header_count(self):
        class FakeHeaders(dict):
            def raw_items(self):
                return self.items()

        class FakeResponse:
            status = 200
            headers = FakeHeaders({f"X-H{i}": "v" for i in range(capture.MAX_FETCH_HEADERS + 1)})

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self, _cap):
                return b"{}"

        with patch.object(capture, "_urlopen_no_redirect", lambda _request, timeout: FakeResponse()):
            with self.assertRaisesRegex(RuntimeError, "too many HTTP headers"):
                capture._fetch_response("https://api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250", 5)

    def test_worker_packet_is_length_checked_and_preserves_receipt_metadata(self):
        body = b'{"synthetic":true}'
        metadata = capture.json.dumps({"headers": {"ETag": "synthetic"}, "status": 200,
                                       "received_ms": 1_795_557_600_100,
                                       "received_mono_ns": 90}, separators=(",", ":")).encode()
        packet = capture.FETCH_MAGIC + capture.struct.pack("<II", len(metadata), len(body)) + metadata + body
        worker = "import sys; sys.stdout.buffer.write(" + repr(packet) + ")"
        result = capture.run_fetch_worker([capture.sys.executable, "-c", worker], 5)
        self.assertEqual(result, (body, {"ETag": "synthetic"}, 200, 1_795_557_600_100, 90))

    def test_fetch_total_deadline_covers_a_stalled_response_body_and_reaps_worker(self):
        real_popen = capture.subprocess.Popen
        children = []

        def track_child(*command, **options):
            child = real_popen(*command, **options)
            children.append(child)
            return child

        stalled_worker = (
            "import time\n"
            "class FakeResponse:\n"
            " def read(self, _limit): time.sleep(10)\n"
            "FakeResponse().read(1048577)\n"
        )
        url = "https://api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250"
        with patch.object(capture, "fetch_worker_command", return_value=[capture.sys.executable, "-c", stalled_worker]), \
             patch.object(capture.subprocess, "Popen", side_effect=track_child):
            started = capture.time.monotonic()
            with self.assertRaises(capture.subprocess.TimeoutExpired):
                capture.fetch_once(url, 0.5)
            elapsed = capture.time.monotonic() - started
        self.assertLess(elapsed, 3.0)
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll())
        self.assertTrue(children[0].stdin.closed)
        self.assertTrue(children[0].stdout.closed)
        self.assertTrue(children[0].stderr.closed)

    def test_adsb_worker_rejects_nonfixed_query(self):
        with self.assertRaisesRegex(ValueError, "fixed ZAU query"):
            capture.fetch_worker_command("https://api.adsb.lol/v2/lat/42.00000/lon/-88.00000/dist/100", 5, False)


if __name__ == "__main__":
    unittest.main()
