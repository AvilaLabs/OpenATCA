"""Print the comparison base used by the Rust proof workflow selector."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys


ZERO_SHA = "0" * 40
SHA_RE = re.compile(r"^[0-9a-fA-F]{40,64}$")


def _git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=False, timeout=30,
    )
    if result.returncode != 0:
        raise ValueError("cannot resolve proof comparison base")
    return result.stdout.strip()


def _commit(repo: Path, revision: str) -> str:
    if not SHA_RE.fullmatch(revision):
        raise ValueError("comparison revision is not a full commit ID")
    return _git(repo, "rev-parse", "--verify", f"{revision}^{{commit}}")


def _merge_base(repo: Path, tip: str, base_ref: str) -> str:
    candidate = _git(repo, "merge-base", tip, base_ref)
    return _commit(repo, candidate)


def comparison_base(
    repo: Path, event: str, ref: str, tip: str, push_before: str, pull_request_base: str,
) -> str:
    """Resolve a workflow comparison base; empty means a full registry run."""
    if event == "pull_request":
        return _commit(repo, pull_request_base)
    if event != "push":
        return ""
    tip_commit = _commit(repo, tip)
    if ref.startswith("refs/heads/train/"):
        return _merge_base(repo, tip_commit, "refs/remotes/origin/main")
    if ref == "refs/heads/main":
        if push_before == ZERO_SHA:
            return _merge_base(repo, tip_commit, "refs/remotes/origin/main")
        return _commit(repo, push_before)
    raise ValueError("unsupported proof workflow push ref")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        repo = args.repo.resolve(strict=True)
        base = comparison_base(
            repo,
            os.environ.get("GITHUB_EVENT_NAME", ""),
            os.environ.get("GITHUB_REF", ""),
            os.environ.get("GITHUB_SHA", ""),
            os.environ.get("GITHUB_EVENT_BEFORE", ""),
            os.environ.get("GITHUB_PR_BASE_SHA", ""),
        )
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        print(f"cannot choose Rust proof comparison base: {exc}", file=sys.stderr)
        return 2
    print(base)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
