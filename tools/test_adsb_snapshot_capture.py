"""Synthetic-only tests for the bounded ADS-B capture wrapper; no network or FAA data."""

import importlib.util
from argparse import Namespace
from contextlib import contextmanager
from pathlib import Path


SCRIPT = Path(__file__).parent / "oa-stdds-ref" / "capture_adsb_lol.py"
SPEC = importlib.util.spec_from_file_location("capture_adsb_lol", SCRIPT)
capture = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(capture)


@contextmanager
def raises(error_type, match=None):
    try:
        yield
    except error_type as exc:
        if match is not None:
            assert match in str(exc)
    else:
        raise AssertionError(f"expected {error_type.__name__}")


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


def test_endpoint_is_regional_v2_and_radius_is_explicit():
    assert capture.request_url(42.5, -88.5, 250) == (
        "https://api.adsb.lol/v2/lat/42.50000/lon/-88.50000/dist/250"
    )
    assert capture.metar_url("KORD,KMDW") == (
        "https://aviationweather.gov/api/data/metar?ids=KORD,KMDW&format=json"
    )


def test_cohort_bounds_are_strict_and_duration_is_bounded():
    assert capture.checked_options(args()) == (1795557600000, 1795557900000)
    try:
        capture.checked_options(args(cohort_to_utc="2026-11-24T22:00:00Z"))
    except ValueError as exc:
        assert "before" in str(exc)
    else:
        raise AssertionError("equal cohort endpoints were accepted")


def test_cohort_times_keep_exact_millisecond_precision_and_reject_submillisecond():
    assert capture.parse_utc("2026-11-24T22:00:00.125Z") == 1_795_557_600_125
    assert capture.parse_utc("2026-11-25T00:00:00.125+02:00") == 1_795_557_600_125
    with raises(ValueError, match="millisecond"):
        capture.parse_utc("2026-11-24T22:00:00.0001Z")


def test_station_count_and_decoder_fixed_query_are_bounded():
    with raises(ValueError, match="four-letter"):
        capture.checked_options(args(metar_stations=",".join(["KORD"] * 33)))
    with raises(ValueError, match="fixed ZAU"):
        capture.checked_options(args(lat=40.0))


def test_guarded_exclusive_writer_and_hardlink_publication(tmp_path):
    guard = capture.sealed_guard.Guard(text="root /tmp\nseal forbidden\nwindow 2026-11-20T00:00:00Z 2026-11-21T00:00:00Z\n")
    source = tmp_path / "capture.part"
    destination = tmp_path / "capture.bin"
    capture.create_exclusive(source, b"synthetic", guard)
    capture.publish_exclusive(source, destination, guard)
    assert not source.exists()
    with guard.open(destination, "rb") as stream:
        assert stream.read() == b"synthetic"
    with raises(FileExistsError):
        capture.create_exclusive(destination, b"overwrite attempt", guard)
    assert destination.read_bytes() == b"synthetic"


def test_guard_refuses_sealed_source_and_cohort_or_envelope_times(tmp_path):
    guard = capture.sealed_guard.Guard(text="root /tmp\nseal forbidden\nwindow 2026-11-24T22:00:00Z 2026-11-24T22:05:00Z\n")
    sealed = tmp_path / "forbidden" / "input.bin"
    with raises(capture.sealed_guard.SealedError):
        capture.create_exclusive(sealed, b"synthetic", guard)
    with raises(RuntimeError, match="cohort interval"):
        capture.check_cohort(guard, 1_795_557_600_000, 1_795_557_900_000)
    body = b'{"ac":[],"ctime":1795557600000,"msg":"No error","now":1795557600000,"ptime":1,"total":0}'
    with raises(RuntimeError, match="sealed window"):
        capture.check_snapshot_times(body, 1_795_558_860_000, guard)


def test_decoder_output_is_stopped_at_declared_caps():
    command = [capture.sys.executable, "-c", "import sys; sys.stdout.write('x' * 4096)"]
    with raises(RuntimeError, match="stdout/stderr cap"):
        capture.bounded_process(command, b"", 5, 128, 128)


def test_response_age_rate_and_body_limits_are_enforced():
    for override, expected in [
        ({"poll_s": 4}, "poll interval"),
        ({"duration_s": 301}, "duration"),
        ({"max_age_ms": 30_001}, "max age"),
        ({"radius_nm": 251}, "radius"),
        ({"duration_s": 301}, "duration"),
        ({"cohort_to_utc": "2026-11-24T22:04:00Z"}, "duration"),
    ]:
        try:
            capture.checked_options(args(**override))
        except ValueError as exc:
            assert expected in str(exc)
        else:
            raise AssertionError(f"invalid options accepted: {override}")


def test_fetch_receipt_is_sampled_after_body_and_headers_are_saved(monkeypatch):
    class FakeResponse:
        status = 200
        headers = {"Date": "Tue, 24 Nov 2026 22:00:00 GMT", "ETag": '"synthetic"', "Content-Type": "application/json"}

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def read(self, cap):
            assert cap == capture.MAX_BODY_BYTES + 1
            return b'{"ac":[],"ctime":1795557600000,"msg":"No error","now":1795557600000,"ptime":1,"total":0}'

    monkeypatch.setattr(capture, "urlopen", lambda _request, timeout: FakeResponse())
    monkeypatch.setattr(capture.time, "monotonic_ns", lambda: 90)
    monkeypatch.setattr(capture, "utc_ms", lambda: 1_795_557_600_100)
    body, headers, status, received_ms, mono = capture.fetch_once("https://synthetic.invalid", 5)
    assert body.startswith(b'{"ac"')
    assert headers["ETag"] == '"synthetic"'
    assert status == 200
    assert (received_ms, mono) == (1_795_557_600_100, 90)


def test_oversized_response_is_refused_without_network(monkeypatch):
    class FakeResponse:
        status = 200
        headers = {}

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def read(self, _cap):
            return b"x" * (capture.MAX_BODY_BYTES + 1)

    monkeypatch.setattr(capture, "urlopen", lambda _request, timeout: FakeResponse())
    try:
        capture.fetch_once("https://synthetic.invalid", 5)
    except RuntimeError as exc:
        assert "exceeds" in str(exc)
    else:
        raise AssertionError("oversized response was accepted")
