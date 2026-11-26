from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from email.message import Message
import importlib.util
import io
import os
from pathlib import Path
import unittest
from unittest import mock
from urllib.error import HTTPError
from urllib.parse import urlsplit


SCRIPT = Path(__file__).with_name("check_python_release_preflight.py")
SPEC = importlib.util.spec_from_file_location("release_preflight", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load release preflight checker")
release_preflight = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release_preflight)


COMMIT = "a" * 40
OTHER_COMMIT = "b" * 40
TAG = "openatca-python-v0.1.0"
ENV = {
    "GITHUB_REPOSITORY": "AvilaLabs/OpenATCA",
    "GITHUB_REF": f"refs/tags/{TAG}",
    "RELEASE_TAG": TAG,
    "EXPECTED_COMMIT": COMMIT,
    "GH_API_TOKEN": "synthetic-token-never-printed",
}


def _run(run_number: int = 17, conclusion: str = "success") -> dict[str, object]:
    return {
        "head_sha": COMMIT,
        "head_branch": "main",
        "event": "push",
        "run_number": run_number,
        "run_attempt": 1,
        "status": "completed",
        "conclusion": conclusion,
    }


def _fake_api(*, annotated: bool = False, ci_runs: list[dict[str, object]] | None = None,
              total: int = 1, ref_targets: list[str] | None = None):
    refs = iter(ref_targets or [COMMIT, COMMIT])
    tag_calls = 0
    ref_calls = 0

    def get_json(path: str, _token: str) -> dict[str, object]:
        nonlocal ref_calls, tag_calls
        if "/git/ref/tags/" in path:
            ref_calls += 1
            target = next(refs)
            return {"object": {"type": "tag" if annotated else "commit",
                               "sha": "c" * 40 if annotated else target}}
        if "/git/tags/" in path:
            tag_calls += 1
            return {"object": {"type": "commit", "sha": COMMIT}}
        if "/actions/workflows/" in path:
            return {"total_count": total, "workflow_runs": ci_runs or [_run()]}
        raise AssertionError(f"unexpected API path {path}")

    return get_json, lambda: (ref_calls, tag_calls)


class ReleasePreflightTests(unittest.TestCase):
    def run_main(self, get_json) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with (mock.patch.dict(os.environ, ENV, clear=True),
              mock.patch.object(release_preflight, "_get_json", side_effect=get_json),
              redirect_stdout(out), redirect_stderr(err)):
            result = release_preflight.main()
        return result, out.getvalue(), err.getvalue()

    def test_success_for_lightweight_tag_checks_and_rechecks_both_workflows(self) -> None:
        get_json, counts = _fake_api()
        result, out, err = self.run_main(get_json)
        self.assertEqual(result, 0, err)
        self.assertIn("ci.yml run=17 attempt=1 success", out)
        self.assertIn("rust-proofs.yml run=17 attempt=1 success", out)
        self.assertEqual(counts(), (2, 0))

    def test_success_resolves_annotated_tag(self) -> None:
        get_json, counts = _fake_api(annotated=True)
        result, out, err = self.run_main(get_json)
        self.assertEqual(result, 0, err)
        self.assertIn("release preflight passed", out)
        self.assertEqual(counts(), (2, 2))

    def test_refuses_retargeted_tag_before_querying_ci(self) -> None:
        get_json, _ = _fake_api(ref_targets=[OTHER_COMMIT])
        result, out, err = self.run_main(get_json)
        self.assertEqual(result, 2)
        self.assertEqual(out, "")
        self.assertIn("retargeted", err)

    def test_refuses_bad_ci_conclusion(self) -> None:
        get_json, _ = _fake_api(ci_runs=[_run(conclusion="failure")])
        result, _, err = self.run_main(get_json)
        self.assertEqual(result, 2)
        self.assertIn("ci.yml", err)

    def test_refuses_newer_failed_run_even_if_older_run_succeeded(self) -> None:
        get_json, _ = _fake_api(total=2, ci_runs=[_run(16), _run(17, "failure")])
        result, _, err = self.run_main(get_json)
        self.assertEqual(result, 2)
        self.assertIn("latest ci.yml run", err)

    def test_refuses_oversized_run_listing(self) -> None:
        get_json, _ = _fake_api(total=101)
        result, _, err = self.run_main(get_json)
        self.assertEqual(result, 2)
        self.assertIn("oversized", err)

    def test_refuses_incomplete_run_listing(self) -> None:
        get_json, _ = _fake_api(total=2, ci_runs=[_run()])
        result, _, err = self.run_main(get_json)
        self.assertEqual(result, 2)
        self.assertIn("incomplete", err)

    def test_refuses_oversized_api_body(self) -> None:
        class Response:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def geturl(self):
                return "https://api.github.com/repos/AvilaLabs/OpenATCA/test"

            def read(self, size: int) -> bytes:
                return b"x" * size

        class Opener:
            def open(self, _request, timeout: int):
                self.timeout = timeout
                return Response()

        opener = Opener()
        with mock.patch.object(release_preflight, "build_opener", return_value=opener):
            with self.assertRaisesRegex(release_preflight.GateError, "byte limit"):
                release_preflight._get_json("/repos/AvilaLabs/OpenATCA/test", "synthetic-token")
        self.assertEqual(opener.timeout, 15)

    def test_redirect_is_refused_before_any_second_host_request(self) -> None:
        requested_hosts: list[str] = []

        class RedirectOpener:
            def __init__(self, handler):
                self.handler = handler

            def open(self, request, timeout: int):
                self.timeout = timeout
                requested_hosts.append(urlsplit(request.full_url).hostname or "")
                redirected = self.handler.redirect_request(
                    request, io.BytesIO(b""), 302, "Found", {"Location": "https://example.invalid/steal"},
                    "https://example.invalid/steal",
                )
                if redirected is not None:
                    requested_hosts.append(urlsplit(redirected.full_url).hostname or "")
                error = HTTPError(request.full_url, 302, "redirect refused", Message(), None)
                error.close()
                raise error

        def opener_factory(handler):
            self.assertIsInstance(handler, release_preflight._NoRedirect)
            return RedirectOpener(handler)

        with mock.patch.object(release_preflight, "build_opener", side_effect=opener_factory):
            with self.assertRaisesRegex(release_preflight.GateError, "HTTP 302"):
                release_preflight._get_json("/repos/AvilaLabs/OpenATCA/test", "synthetic-token")
        self.assertEqual(requested_hosts, ["api.github.com"])

    def test_refuses_wrong_repository_before_network(self) -> None:
        environment = dict(ENV, GITHUB_REPOSITORY="other/repo")
        with (mock.patch.dict(os.environ, environment, clear=True),
              mock.patch.object(release_preflight, "_get_json") as request):
            result = release_preflight.main()
        self.assertEqual(result, 2)
        request.assert_not_called()


if __name__ == "__main__":
    unittest.main()
