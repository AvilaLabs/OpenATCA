"""Fail-closed release gate for the exact commit behind a Python package tag."""

from __future__ import annotations

import json
import os
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


API_ROOT = "https://api.github.com"
EXPECTED_REPOSITORY = "AvilaLabs/OpenATCA"
WORKFLOWS = ("ci.yml", "rust-proofs.yml")
MAX_RESPONSE_BYTES = 1_000_000
MAX_TAG_DEPTH = 5
TAG_PATTERN = re.compile(r"openatca-python-v[0-9]+\.[0-9]+\.[0-9]+\Z")
SHA_PATTERN = re.compile(r"[0-9a-f]{40}\Z")


class GateError(Exception):
    """A missing or unverifiable release condition."""


class _NoRedirect(HTTPRedirectHandler):
    """Refuse redirects before urllib can forward an authenticated request."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


def _environment() -> tuple[str, str, str, str]:
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    tag = os.environ.get("RELEASE_TAG", "")
    commit = os.environ.get("EXPECTED_COMMIT", "")
    token = os.environ.get("GH_API_TOKEN", "")
    if repository != EXPECTED_REPOSITORY:
        raise GateError("workflow repository does not match the approved repository")
    if not TAG_PATTERN.fullmatch(tag):
        raise GateError("release tag does not match openatca-python-vMAJOR.MINOR.PATCH")
    if not SHA_PATTERN.fullmatch(commit):
        raise GateError("release event does not provide a canonical 40-character commit SHA")
    if not token:
        raise GateError("GitHub API token is unavailable")
    if os.environ.get("GITHUB_REF") != f"refs/tags/{tag}":
        raise GateError("workflow ref is not the published package tag")
    return repository, tag, commit, token


def _get_json(path: str, token: str) -> dict[str, object]:
    if not path.startswith("/repos/AvilaLabs/OpenATCA/") or ".." in path.split("/"):
        raise GateError("refused unexpected GitHub API path")
    request = Request(
        API_ROOT + path,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "OpenATCA-Python-release-check",
        },
    )
    try:
        with build_opener(_NoRedirect()).open(request, timeout=15) as response:
            if urlsplit(response.geturl()).hostname != "api.github.com":
                raise GateError("GitHub API redirected outside api.github.com")
            body = response.read(MAX_RESPONSE_BYTES + 1)
    except HTTPError as exc:
        raise GateError(f"GitHub API returned HTTP {exc.code}") from None
    except (URLError, TimeoutError, OSError):
        raise GateError("GitHub API request failed") from None
    if len(body) > MAX_RESPONSE_BYTES:
        raise GateError("GitHub API response exceeded the release-check byte limit")
    try:
        value = json.loads(body)
    except (json.JSONDecodeError, UnicodeDecodeError):
        raise GateError("GitHub API returned invalid JSON") from None
    if not isinstance(value, dict):
        raise GateError("GitHub API response was not an object")
    return value


def _tag_commit(tag: str, token: str) -> str:
    ref = _get_json(f"/repos/{EXPECTED_REPOSITORY}/git/ref/tags/{quote(tag, safe='')}", token)
    obj = ref.get("object")
    if not isinstance(obj, dict):
        raise GateError("GitHub tag reference has no object")
    for _ in range(MAX_TAG_DEPTH):
        object_type = obj.get("type")
        object_sha = obj.get("sha")
        if not isinstance(object_sha, str) or not SHA_PATTERN.fullmatch(object_sha):
            raise GateError("GitHub tag object has an invalid SHA")
        if object_type == "commit":
            return object_sha
        if object_type != "tag":
            raise GateError("release tag does not resolve to a commit")
        tag_object = _get_json(f"/repos/{EXPECTED_REPOSITORY}/git/tags/{object_sha}", token)
        obj = tag_object.get("object")
        if not isinstance(obj, dict):
            raise GateError("annotated tag has no target object")
    raise GateError("annotated tag nesting exceeded the release-check limit")


def _latest_push_run(workflow: str, commit: str, token: str) -> dict[str, object]:
    path = (
        f"/repos/{EXPECTED_REPOSITORY}/actions/workflows/{quote(workflow, safe='')}/runs"
        f"?head_sha={commit}&per_page=100"
    )
    response = _get_json(path, token)
    total = response.get("total_count")
    runs = response.get("workflow_runs")
    if type(total) is not int or not isinstance(runs, list):
        raise GateError(f"{workflow} returned an invalid run list")
    if total > 100:
        raise GateError(f"{workflow} returned an oversized run list")
    if len(runs) != total:
        raise GateError(f"{workflow} returned an incomplete run list")
    matches: list[dict[str, object]] = []
    for run in runs:
        if not isinstance(run, dict):
            raise GateError(f"{workflow} returned a malformed workflow run")
        if (run.get("head_sha") == commit and run.get("head_branch") == "main" and
                run.get("event") == "push"):
            number = run.get("run_number")
            attempt = run.get("run_attempt")
            if type(number) is not int or type(attempt) is not int:
                raise GateError(f"{workflow} run has invalid ordering fields")
            matches.append(run)
    if not matches:
        raise GateError(f"{workflow} has no main-branch push run for the release commit")
    latest = max(matches, key=lambda run: (run["run_number"], run["run_attempt"]))
    if latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise GateError(f"latest {workflow} run for the release commit is not successful")
    return latest


def main() -> int:
    try:
        repository, tag, commit, token = _environment()
        actual = _tag_commit(tag, token)
        if actual != commit:
            raise GateError("release tag was retargeted from the event commit")
        summaries = []
        for workflow in WORKFLOWS:
            run = _latest_push_run(workflow, commit, token)
            summaries.append(f"{workflow} run={run['run_number']} attempt={run['run_attempt']} success")
        if _tag_commit(tag, token) != commit:
            raise GateError("release tag changed during preflight")
        print(f"release preflight passed: {repository} {tag} commit={commit}")
        for summary in summaries:
            print(summary)
        return 0
    except GateError as exc:
        print(f"release preflight refused: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
