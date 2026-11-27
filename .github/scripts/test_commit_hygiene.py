from __future__ import annotations

import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("commit_hygiene", Path(__file__).with_name("check_commit_hygiene.py"))
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


def git_bytes(repo: Path, *args: str, input_bytes: bytes | None = None) -> bytes:
    result = subprocess.run(
        ["git", *args], cwd=repo, input=input_bytes, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, check=True,
    )
    return result.stdout.strip()


def make_tree(repo: Path, files: dict[str, bytes]) -> str:
    root: dict = {}
    for path, contents in files.items():
        node = root
        components = path.split("/")
        for component in components[:-1]:
            node = node.setdefault(component, {})
        node[components[-1]] = contents

    def write_node(node: dict) -> str:
        entries = []
        for name, value in sorted(node.items()):
            if isinstance(value, dict):
                entries.append(f"040000 tree {write_node(value)}\t{name}\n".encode())
            else:
                blob = git_bytes(repo, "hash-object", "-w", "--stdin", input_bytes=value).decode()
                entries.append(f"100644 blob {blob}\t{name}\n".encode())
        return git_bytes(repo, "mktree", input_bytes=b"".join(entries)).decode()

    return write_node(root)


def make_commit(repo: Path, tree: str, message: str, parent: str | None = None) -> str:
    lines = [f"tree {tree}"]
    if parent:
        lines.append(f"parent {parent}")
    lines.extend([
        "author Connor Avila <ci-fixture@example.invalid> 1795766400 +0000",
        "committer Connor Avila <ci-fixture@example.invalid> 1795766400 +0000",
        "",
        message,
        "",
    ])
    return git_bytes(repo, "hash-object", "-t", "commit", "-w", "--stdin",
                     input_bytes="\n".join(lines).encode()).decode()


class CommitHygieneTests(unittest.TestCase):
    def test_message_markers_are_case_insensitive(self):
        for marker in module.BAD_MARKERS:
            with self.subTest(marker=marker), patch.object(module, "git", return_value=(
                b"Connor Avila\0x@example.org\0Connor Avila\0x@example.org\0subject " + marker.upper().encode()
            )):
                with self.assertRaises(module.Refusal) as caught:
                    module.check_commit("0123456789abcdef", "x@example.org")
                self.assertEqual(caught.exception.rule, "prohibited-message-marker")

    def test_names_and_both_emails_are_checked(self):
        good = b"Connor Avila\0x@example.org\0Connor Avila\0x@example.org\0safe\n"
        with patch.object(module, "git", return_value=good):
            module.check_commit("0123456789abcdef", "x@example.org")
        for fields, rule in [
            (b"Other Person\0x@example.org\0Connor Avila\0x@example.org\0safe", "author-name"),
            (b"Connor Avila\0x@example.org\0Other Person\0x@example.org\0safe", "committer-name"),
            (b"Connor Avila\0y@example.org\0Connor Avila\0x@example.org\0safe", "author-email"),
            (b"Connor Avila\0x@example.org\0Connor Avila\0y@example.org\0safe", "committer-email"),
        ]:
            with self.subTest(rule=rule), patch.object(module, "git", return_value=fields):
                with self.assertRaises(module.Refusal) as caught:
                    module.check_commit("0123456789abcdef", "x@example.org")
                self.assertEqual(caught.exception.rule, rule)

    def test_bad_or_empty_range_is_refused(self):
        with patch.object(module, "is_commit", return_value=False):
            with self.assertRaises(module.Refusal):
                module.commit_ids("missing", "head", True)
        def range_run(args, **_kwargs):
            if "--is-ancestor" in args:
                return SimpleNamespace(returncode=1, stdout=b"")
            return SimpleNamespace(returncode=0, stdout=b"merge-base\n")
        with patch.object(module, "is_commit", return_value=True), patch.object(
            module.subprocess, "run", side_effect=range_run
        ), patch.object(module, "git", return_value=b"1234567890abcdef\n"):
            with self.assertRaises(module.Refusal) as caught:
                module.commit_ids("base", "head", True)
            self.assertEqual(caught.exception.rule, "non-ancestor-range")
        with patch.object(module, "is_commit", return_value=True), patch.object(
            module.subprocess, "run", side_effect=range_run
        ), patch.object(module, "git", return_value=b""):
            with self.assertRaises(module.Refusal) as caught:
                module.commit_ids("base", "head", False)
            self.assertEqual(caught.exception.rule, "empty-range")

    def test_pull_request_range_allows_a_diverged_branch_with_a_common_base(self):
        def range_run(args, **_kwargs):
            if "--is-ancestor" in args:
                return SimpleNamespace(returncode=1, stdout=b"")
            return SimpleNamespace(returncode=0, stdout=b"merge-base\n")
        with patch.object(module, "is_commit", return_value=True), patch.object(
            module.subprocess, "run", side_effect=range_run
        ), patch.object(module, "git", return_value=b"deadbeef\n"):
            self.assertEqual(module.commit_ids("base", "head", False), ["deadbeef"])

    def test_missing_email_configuration_fails_before_git_or_ref_resolution(self):
        head = "a" * 40
        with patch.dict(os.environ, {
            "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": head,
        }, clear=True):
            with self.assertRaises(module.Refusal) as caught:
                module.main()
        self.assertEqual(caught.exception.rule, "missing-or-invalid-email-variable")
        self.assertEqual(caught.exception.sha, head[:12])

    def test_failure_output_is_only_valid_short_sha_and_rule(self):
        head = "b" * 40
        with patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push", "GITHUB_SHA": head}):
            self.assertEqual(
                module.failure_line(module.Refusal("author-email")),
                f"commit_hygiene_failed sha={head[:12]} rule=author-email",
            )
        pr_head = "c" * 40
        with patch.dict(os.environ, {
            "GITHUB_EVENT_NAME": "pull_request", "GITHUB_SHA": "d" * 40,
            "PR_HEAD_SHA": pr_head,
        }):
            self.assertEqual(module.Refusal("invalid-range").sha, pr_head[:12])
        unsafe = "untrusted-value-that-must-not-be-printed"
        with patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push", "GITHUB_SHA": unsafe}):
            refusal = module.Refusal("invalid-range", unsafe)
            self.assertEqual(refusal.sha, "000000000000")
            self.assertNotIn(unsafe, module.failure_line(refusal))

    def test_tracked_user_path_match_is_refused_and_no_match_passes(self):
        with patch.object(module.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
            with self.assertRaises(module.Refusal) as caught:
                module.check_tracked_paths(["0123456789abcdef"])
            self.assertEqual(caught.exception.rule, "tracked-user-path")
        with patch.object(module.subprocess, "run", return_value=SimpleNamespace(returncode=1)):
            module.check_tracked_paths(["0123456789abcdef"])

    def test_git_range_metadata_and_every_tree_are_checked_without_identity_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            subprocess.run(["git", "init", "--quiet"], cwd=repo, check=True)
            root_commit = make_commit(repo, make_tree(repo, {"source.txt": b"ancestor"}), "base")
            base = make_commit(repo, make_tree(repo, {"source.txt": b"base"}), "base update", root_commit)
            needle = "/" + "home" + "/" + "connoravila"
            bad = make_commit(repo, make_tree(repo, {
                "source.txt": b"feature", "private-reference.txt": needle.encode(),
            }), "feature update", root_commit)
            fixed = make_commit(repo, make_tree(repo, {"source.txt": b"fixed"}), "remove path", bad)
            archived = make_commit(repo, make_tree(repo, {
                "docs/archive/legacy.txt": needle.encode(),
            }), "archive historical sample", fixed)

            pr_ids = module.commit_ids(base, bad, False, cwd=repo)
            self.assertEqual(pr_ids, [bad])
            with self.assertRaises(module.Refusal) as caught:
                module.commit_ids(base, bad, True, cwd=repo)
            self.assertEqual(caught.exception.rule, "non-ancestor-range")
            module.check_commit(bad, "ci-fixture@example.invalid", cwd=repo)
            with self.assertRaises(module.Refusal) as caught:
                module.check_tracked_paths(pr_ids, cwd=repo)
            self.assertEqual(caught.exception.rule, "tracked-user-path")
            self.assertEqual(caught.exception.sha, bad[:12])
            module.check_tracked_paths([fixed], cwd=repo)
            module.check_tracked_paths([archived], cwd=repo)


if __name__ == "__main__":
    unittest.main()
