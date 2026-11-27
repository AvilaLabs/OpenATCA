from __future__ import annotations

import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("commit_hygiene", Path(__file__).with_name("check_commit_hygiene.py"))
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)

FIXTURE_EMAIL = "quality-proof@example.invalid"
AUTHOR_EMAIL_RULE = "author-email"
COMMITTER_EMAIL_RULE = "committer-email"
MESSAGE_RULE = "prohibited-message-marker"
PATH_RULE = "tracked-user-path"


@dataclass(frozen=True)
class CommitFixture:
    message: str
    author_name: str = "Connor Avila"
    author_email: str = FIXTURE_EMAIL
    committer_name: str = "Connor Avila"
    committer_email: str = FIXTURE_EMAIL


@dataclass(frozen=True)
class ProofCase:
    class_id: str
    label: str
    fixture: CommitFixture
    files: dict[str, bytes]
    expected_rule: str


@dataclass(frozen=True)
class ProofReceipt:
    class_id: str
    sha: str
    baseline_exit: int
    checker_exit: int
    refusal: str


class ProofHarnessFailure(Exception):
    def __init__(self, reason: str):
        self.reason = reason

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


def make_commit(repo: Path, tree: str, fixture: CommitFixture, parent: str | None = None) -> str:
    lines = [f"tree {tree}"]
    if parent:
        lines.append(f"parent {parent}")
    lines.extend([
        f"author {fixture.author_name} <{fixture.author_email}> 1795766400 +0000",
        f"committer {fixture.committer_name} <{fixture.committer_email}> 1795766400 +0000",
        "",
        fixture.message,
        "",
    ])
    return git_bytes(repo, "hash-object", "-t", "commit", "-w", "--stdin",
                     input_bytes="\n".join(lines).encode()).decode()


def proof_cases() -> dict[str, tuple[ProofCase, ...]]:
    path_marker = "/" + "home" + "/" + "connoravila"
    return {
        "E01": (
            ProofCase("E01", "author-email", CommitFixture("synthetic identity case", author_email="author@example.invalid"),
                      {"fixture.txt": b"safe"}, AUTHOR_EMAIL_RULE),
            ProofCase("E01", "committer-email", CommitFixture("synthetic identity case", committer_email="committer@example.invalid"),
                      {"fixture.txt": b"safe"}, COMMITTER_EMAIL_RULE),
        ),
        "E02": (
            ProofCase("E02", "prohibited-attribution", CommitFixture("synthetic footer generated with fixture tool"),
                      {"fixture.txt": b"safe"}, MESSAGE_RULE),
        ),
        "E03": (
            ProofCase("E03", "tracked-local-path-in-archive", CommitFixture("synthetic path case"),
                      {"docs/archive/fixture.txt": path_marker.encode()}, PATH_RULE),
        ),
    }


def run_proof_case(case: ProofCase) -> ProofReceipt:
    try:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            subprocess.run(["git", "init", "--quiet"], cwd=repo, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            base_tree = make_tree(repo, {"fixture.txt": b"baseline"})
            base = make_commit(repo, base_tree, CommitFixture("synthetic safe baseline"))
            safe_head = make_commit(
                repo, base_tree, CommitFixture("synthetic safe positive control"), base
            )
            bad_tree = make_tree(repo, case.files)
            head = make_commit(repo, bad_tree, case.fixture, safe_head)
            test_env = {
                "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                "COMMIT_EMAIL_LOG_MASK": FIXTURE_EMAIL,
                "EXPECTED_COMMIT_EMAIL": FIXTURE_EMAIL,
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_REF": "refs/heads/main",
                "GITHUB_SHA": safe_head,
                "PUSH_BEFORE_SHA": base,
            }
            checker = Path(__file__).with_name("check_commit_hygiene.py").resolve()
            baseline = subprocess.run(
                [sys.executable, str(checker)], cwd=repo, env=test_env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            if baseline.returncode != 0 or baseline.stdout or baseline.stderr:
                raise ProofHarnessFailure("positive-control")
            test_env["GITHUB_SHA"] = head
            test_env["PUSH_BEFORE_SHA"] = safe_head
            result = subprocess.run(
                [sys.executable, str(checker)], cwd=repo, env=test_env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            expected_line = f"commit_hygiene_failed sha={head[:12]} rule={case.expected_rule}"
            captured = result.stdout + result.stderr
            if result.returncode != 1:
                raise ProofHarnessFailure("checker-exit")
            if result.stdout or result.stderr.strip() != expected_line:
                raise ProofHarnessFailure("refusal-shape")
            if FIXTURE_EMAIL in captured or ("/" + "home" + "/" + "connoravila") in captured:
                raise ProofHarnessFailure("fixture-value-output")
            return ProofReceipt(case.class_id, head[:12], baseline.returncode,
                                result.returncode, case.expected_rule)
    except ProofHarnessFailure:
        raise
    except Exception:
        raise ProofHarnessFailure("fixture-setup") from None


def run_proof_class(class_id: str) -> tuple[ProofReceipt, ...]:
    cases = proof_cases().get(class_id)
    if cases is None:
        raise ProofHarnessFailure("unknown-class")
    return tuple(run_proof_case(case) for case in cases)


def proof_command(class_id: str) -> int:
    safe_class = class_id if class_id in proof_cases() else "unknown"
    try:
        receipts = run_proof_class(class_id)
    except ProofHarnessFailure as failure:
        print(f"proof_harness=failed class={safe_class} reason={failure.reason}", file=sys.stderr)
        return 2
    for receipt in receipts:
        print(
            f"proof_fixture=validated class={receipt.class_id} baseline_exit={receipt.baseline_exit} "
            f"checker_exit={receipt.checker_exit} "
            f"expected_refusal={receipt.refusal} observed_refusal={receipt.refusal} fixture_sha={receipt.sha}"
        )
    return receipts[0].checker_exit


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
            "COMMIT_EMAIL_LOG_MASK": "fixture@example.invalid",
        }, clear=True):
            with self.assertRaises(module.Refusal) as caught:
                module.main()
        self.assertEqual(caught.exception.rule, "missing-or-invalid-email-variable")
        self.assertEqual(caught.exception.sha, head[:12])

    def test_missing_or_mismatched_redaction_carrier_fails_before_git(self):
        with patch.dict(os.environ, {"EXPECTED_COMMIT_EMAIL": "fixture@example.invalid"}, clear=True):
            with self.assertRaises(module.Refusal) as caught:
                module.main(mask_preflight=True)
        self.assertEqual(caught.exception.rule, "email-log-mask-missing")
        with patch.dict(os.environ, {
            "COMMIT_EMAIL_LOG_MASK": "fixture@example.invalid",
            "COMMIT_EMAIL_REDACTION_READY": "true",
        }, clear=True):
            self.assertEqual(module.main(mask_preflight=True), 0)
        with patch.dict(os.environ, {
            "COMMIT_EMAIL_LOG_MASK": "fixture@example.invalid",
            "COMMIT_EMAIL_REDACTION_READY": "false",
        }, clear=True):
            with self.assertRaises(module.Refusal) as caught:
                module.main(mask_preflight=True)
        self.assertEqual(caught.exception.rule, "email-log-mask-mismatch")
        with patch.dict(os.environ, {
            "EXPECTED_COMMIT_EMAIL": "fixture@example.invalid",
            "COMMIT_EMAIL_LOG_MASK": "different@example.invalid",
        }, clear=True):
            with self.assertRaises(module.Refusal) as caught:
                module.main()
        self.assertEqual(caught.exception.rule, "email-log-mask-mismatch")

    def test_workflow_uses_secret_carrier_and_vars_only_for_validation(self):
        workflow = (Path(__file__).parents[1] / "workflows" / "ci.yml").read_text()
        self.assertIn("COMMIT_EMAIL_LOG_MASK: ${{ secrets.COMMIT_EMAIL_LOG_MASK }}", workflow)
        self.assertIn(
            "COMMIT_EMAIL_REDACTION_READY: ${{ vars.COMMIT_EMAIL == secrets.COMMIT_EMAIL_LOG_MASK }}",
            workflow,
        )
        self.assertIn("EXPECTED_COMMIT_EMAIL: ${{ vars.COMMIT_EMAIL }}", workflow)
        self.assertIn(
            "if: env.COMMIT_EMAIL_LOG_MASK != '' && env.COMMIT_EMAIL_REDACTION_READY == 'true'",
            workflow,
        )
        guard_start = workflow.index("- name: Require commit-email log redaction carrier")
        checker_start = workflow.index("- name: Check commit metadata and tracked path hygiene")
        guard_step = workflow[guard_start:checker_start]
        self.assertNotIn("EXPECTED_COMMIT_EMAIL", guard_step)
        self.assertIn("--mask-preflight", guard_step)
        self.assertLess(guard_start, checker_start)

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
            root_commit = make_commit(
                repo, make_tree(repo, {"source.txt": b"ancestor"}), CommitFixture("base")
            )
            base = make_commit(
                repo, make_tree(repo, {"source.txt": b"base"}), CommitFixture("base update"), root_commit
            )
            needle = "/" + "home" + "/" + "connoravila"
            bad = make_commit(repo, make_tree(repo, {
                "source.txt": b"feature", "private-reference.txt": needle.encode(),
            }), CommitFixture("feature update"), root_commit)
            fixed = make_commit(
                repo, make_tree(repo, {"source.txt": b"fixed"}), CommitFixture("remove path"), bad
            )
            archived = make_commit(repo, make_tree(repo, {
                "docs/archive/legacy.txt": needle.encode(),
            }), CommitFixture("archive historical sample"), fixed)

            pr_ids = module.commit_ids(base, bad, False, cwd=repo)
            self.assertEqual(pr_ids, [bad])
            with self.assertRaises(module.Refusal) as caught:
                module.commit_ids(base, bad, True, cwd=repo)
            self.assertEqual(caught.exception.rule, "non-ancestor-range")
            module.check_commit(bad, FIXTURE_EMAIL, cwd=repo)
            with self.assertRaises(module.Refusal) as caught:
                module.check_tracked_paths(pr_ids, cwd=repo)
            self.assertEqual(caught.exception.rule, "tracked-user-path")
            self.assertEqual(caught.exception.sha, bad[:12])
            module.check_tracked_paths([fixed], cwd=repo)
            with self.assertRaises(module.Refusal) as caught:
                module.check_tracked_paths([archived], cwd=repo)
            self.assertEqual(caught.exception.rule, PATH_RULE)

    def test_E01_synthetic_author_and_committer_email_mutations_are_refused(self):
        receipts = run_proof_class("E01")
        self.assertEqual(tuple(receipt.refusal for receipt in receipts),
                         (AUTHOR_EMAIL_RULE, COMMITTER_EMAIL_RULE))
        self.assertTrue(all(receipt.baseline_exit == 0 for receipt in receipts))
        self.assertTrue(all(receipt.checker_exit == 1 for receipt in receipts))

    def test_E02_synthetic_attribution_message_mutation_is_refused(self):
        receipts = run_proof_class("E02")
        self.assertEqual(tuple(receipt.refusal for receipt in receipts), (MESSAGE_RULE,))
        self.assertTrue(all(receipt.baseline_exit == 0 for receipt in receipts))
        self.assertTrue(all(receipt.checker_exit == 1 for receipt in receipts))

    def test_E03_synthetic_archive_path_mutation_is_refused(self):
        receipts = run_proof_class("E03")
        self.assertEqual(tuple(receipt.refusal for receipt in receipts), (PATH_RULE,))
        self.assertTrue(all(receipt.baseline_exit == 0 for receipt in receipts))
        self.assertTrue(all(receipt.checker_exit == 1 for receipt in receipts))

    def test_proof_command_propagates_checker_refusal_only_after_validated_receipt(self):
        from contextlib import redirect_stdout

        output = io.StringIO()
        with redirect_stdout(output):
            exit_code = proof_command("E02")
        self.assertEqual(exit_code, 1)
        lines = output.getvalue().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertTrue(lines[0].startswith(
            "proof_fixture=validated class=E02 baseline_exit=0 checker_exit=1 "
            f"expected_refusal={MESSAGE_RULE} observed_refusal={MESSAGE_RULE} fixture_sha="
        ))
        self.assertNotIn(FIXTURE_EMAIL, output.getvalue())

    def test_proof_cli_returns_the_checker_refusal_for_each_escape_class(self):
        script = Path(__file__).resolve()
        fixture_env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin")}
        expected_rules = {
            "E01": (AUTHOR_EMAIL_RULE, COMMITTER_EMAIL_RULE),
            "E02": (MESSAGE_RULE,),
            "E03": (PATH_RULE,),
        }
        for class_id, rules in expected_rules.items():
            with self.subTest(class_id=class_id):
                result = subprocess.run(
                    [sys.executable, str(script), "--proof-class", class_id],
                    env=fixture_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                )
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stderr, "")
                lines = result.stdout.splitlines()
                self.assertEqual(len(lines), len(rules))
                for line, rule in zip(lines, rules):
                    self.assertTrue(line.startswith(
                        f"proof_fixture=validated class={class_id} baseline_exit=0 checker_exit=1 "
                        f"expected_refusal={rule} observed_refusal={rule} fixture_sha="
                    ))
                self.assertNotIn(FIXTURE_EMAIL, result.stdout + result.stderr)


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--proof-class":
        if len(sys.argv) != 3:
            print("proof_harness=failed class=unknown reason=invalid-arguments", file=sys.stderr)
            raise SystemExit(2)
        raise SystemExit(proof_command(sys.argv[2]))
    unittest.main()
