from __future__ import annotations

import importlib.util
import os
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("commit_hygiene", Path(__file__).with_name("check_commit_hygiene.py"))
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


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
                module.commit_ids("missing", "head")
        with patch.object(module, "is_commit", return_value=True), patch.object(
            module.subprocess, "run", return_value=SimpleNamespace(returncode=1)
        ):
            with self.assertRaises(module.Refusal) as caught:
                module.commit_ids("base", "head")
            self.assertEqual(caught.exception.rule, "non-ancestor-range")
        with patch.object(module, "is_commit", return_value=True), patch.object(
            module.subprocess, "run", return_value=SimpleNamespace(returncode=0)
        ), patch.object(module, "git", return_value=b""):
            with self.assertRaises(module.Refusal) as caught:
                module.commit_ids("base", "head")
            self.assertEqual(caught.exception.rule, "empty-range")

    def test_missing_email_configuration_fails_before_git_or_ref_resolution(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(module.Refusal) as caught:
                module.main()
        self.assertEqual(caught.exception.rule, "missing-or-invalid-email-variable")

    def test_tracked_user_path_match_is_refused_and_no_match_passes(self):
        with patch.object(module.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
            with self.assertRaises(module.Refusal) as caught:
                module.check_tracked_paths("0123456789abcdef")
            self.assertEqual(caught.exception.rule, "tracked-user-path")
        with patch.object(module.subprocess, "run", return_value=SimpleNamespace(returncode=1)):
            module.check_tracked_paths("0123456789abcdef")


if __name__ == "__main__":
    unittest.main()
