"""The commit-spec and argument checks of replay_compare.sh (no build is started)."""

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent / "replay_compare.sh"
OVERLAY_SCRIPT = Path(__file__).resolve().parent / "replay_overlay_apply.sh"
OVERLAY_PATCH = Path(__file__).resolve().parent / "replay_compare_overlay.patch"
REPO = Path(__file__).resolve().parent.parent


def run(*args, cwd=None):
    return subprocess.run(
        ["bash", str(SCRIPT), *args],
        capture_output=True,
        text=True,
        cwd=cwd,
        timeout=60,
    )


class ReplayCompareArguments(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.clip = self.dir / "clip.oaf"
        self.clip.write_bytes(b"x")
        self.out = self.dir / "out"

    def tearDown(self):
        self.tmp.cleanup()

    # REQ: OA-RDF-006
    def test_a_spec_that_is_not_a_hash_or_ref_is_refused_before_anything_is_built(self):
        for bad in [
            "HEAD~1",
            "HEAD^",
            "main..HEAD",
            "v1-dirty",
            "main@{1}",
            "a b",
            "HEAD:file",
            ".",
            "",
        ]:
            with self.subTest(spec=bad):
                r = run(bad, "HEAD", str(self.clip), str(self.out))
                self.assertEqual(r.returncode, 2, r.stderr)
                self.assertIn("not a commit hash or ref", r.stderr)
                self.assertFalse((self.out / "work").exists())
        for opt in ["--help", "-x"]:
            r = run(opt, "HEAD", str(self.clip), str(self.out))
            self.assertEqual(r.returncode, 2)
            self.assertIn("usage", r.stderr)
        r = run("HEAD", "no-such-ref-for-the-test", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such commit", r.stderr)

    # REQ: OA-RDF-006
    def test_missing_clip_wrong_argument_count_and_existing_report_are_refused(self):
        r = run("HEAD", "HEAD", str(self.dir / "nope.oaf"), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such clip", r.stderr)
        r = run("HEAD", "HEAD")
        self.assertEqual(r.returncode, 2)
        self.assertIn("usage", r.stderr)
        r = run("HEAD", "HEAD", str(self.clip), str(self.out), "--expect", "missing.txt")
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such expect file", r.stderr)
        self.out.mkdir()
        (self.out / "report.txt").write_text("old")
        r = run("HEAD", "HEAD", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("exists", r.stderr)
        self.assertEqual((self.out / "report.txt").read_text(), "old")


def git(cwd, *args):
    return subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
        cwd=cwd, capture_output=True, text=True, check=True,
    ).stdout


class OverlayApply(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def repo_with(self, text):
        r = self.dir / "r"
        r.mkdir()
        git(r, "init", "-q")
        (r / "f.txt").write_text(text)
        git(r, "add", "f.txt")
        git(r, "commit", "-q", "-m", "base")
        return r

    def patch_for(self, r):
        (r / "f.txt").write_text("one\nTWO\nthree\nfour\nfive\n")
        patch = git(r, "diff", "--no-color")
        git(r, "checkout", "-q", "--", "f.txt")
        p = self.dir / "p.patch"
        p.write_text(patch)
        return p

    def apply(self, r, p, commit="HEAD"):
        return subprocess.run(
            ["bash", str(OVERLAY_SCRIPT), str(r), commit, str(p), str(self.dir / "rep.txt")],
            capture_output=True, text=True, timeout=60,
        )

    # REQ: OA-RDF-014
    def test_a_patch_applies_only_on_the_blob_it_was_made_against_and_is_recorded(self):
        r = self.repo_with("one\ntwo\nthree\nfour\nfive\n")
        p = self.patch_for(r)
        res = self.apply(r, p)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual((r / "f.txt").read_text(), "one\nTWO\nthree\nfour\nfive\n")
        rep = (self.dir / "rep.txt").read_text()
        self.assertIn("overlay patch sha256: ", rep)
        self.assertRegex(rep, r"patched file sha256: [0-9a-f]{64}  f\.txt")

    # REQ: OA-RDF-014
    def test_a_different_preimage_is_refused_even_when_the_context_would_match(self):
        base = "one\ntwo\nthree\nfour\nfive\n"
        r = self.repo_with(base)
        p = self.patch_for(r)
        other = self.dir / "o"
        other.mkdir()
        git(other, "init", "-q")
        # A file the patch would apply to by context (git apply alone would accept it), but
        # which is not the blob the patch was made against.
        (other / "f.txt").write_text(base + "six\n")
        git(other, "add", "f.txt")
        git(other, "commit", "-q", "-m", "x")
        res = self.apply(other, p)
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("preimage mismatch", res.stderr)
        self.assertEqual((other / "f.txt").read_text(), base + "six\n")
        # A missing file and an existing file the patch creates are refused too.
        empty = self.dir / "e"
        empty.mkdir()
        git(empty, "init", "-q")
        (empty / "g.txt").write_text("x")
        git(empty, "add", "g.txt")
        git(empty, "commit", "-q", "-m", "x")
        res = self.apply(empty, p)
        self.assertEqual(res.returncode, 2)
        self.assertIn("does not exist", res.stderr)

    # REQ: OA-RDF-014
    def test_the_real_overlay_applies_to_the_commit_it_was_made_for_and_not_to_head(self):
        for rev in ["e251fdb"]:
            if subprocess.run(["git", "cat-file", "-e", rev + "^{commit}"], cwd=REPO).returncode != 0:
                self.skipTest("history not available")
        wt = self.dir / "wt"
        git(REPO, "worktree", "add", "--detach", str(wt), "e251fdb")
        try:
            res = self.apply(wt, OVERLAY_PATCH, "e251fdb")
            self.assertEqual(res.returncode, 0, res.stderr)
            self.assertIn("canon.rs", (self.dir / "rep.txt").read_text())
        finally:
            git(REPO, "worktree", "remove", "--force", str(wt))
        head = self.dir / "head"
        git(REPO, "worktree", "add", "--detach", str(head), "HEAD")
        try:
            res = self.apply(head, OVERLAY_PATCH, "HEAD")
            self.assertEqual(res.returncode, 2)
        finally:
            git(REPO, "worktree", "remove", "--force", str(head))

    # REQ: OA-RDF-015
    def test_the_overlay_writes_the_same_canonical_format_as_the_tree(self):
        consts = re.compile(
            r"pub const (ROW_BYTES|OFF_[A-Z_]+|ROWS_VERSION|ROWS_HEADER_BYTES|DIGEST_HEADER|ROWS_MAGIC)\b[^=]*= ([^;]+);"
        )
        patch = OVERLAY_PATCH.read_text()
        canon_in_patch = "\n".join(
            l[1:] for l in patch.splitlines() if l.startswith("+") and not l.startswith("+++")
        )
        now = (REPO / "crates/oa-track/src/canon.rs").read_text()
        a = dict(consts.findall(canon_in_patch))
        b = dict(consts.findall(now))
        self.assertGreater(len(a), 16)
        self.assertEqual(a, b)

    # REQ: OA-RDF-016
    def test_builds_run_locked_with_the_flag_environment_cleared_and_recorded(self):
        # Stub cargo and rustc record what they see; the build is made to fail after that.
        bindir = self.dir / "bin"
        bindir.mkdir()
        log = self.dir / "calls.log"
        stub = "#!/usr/bin/env bash\necho \"$(basename $0) $* RUSTFLAGS=[${RUSTFLAGS-unset}] ENC=[${CARGO_ENCODED_RUSTFLAGS-unset}]\" >>" + str(log) + "\n"
        for name in ["cargo", "rustc"]:
            (bindir / name).write_text(stub + ('echo "rustc 1.89.0 stub"\n' if name == "rustc" else "exit 1\n"))
            (bindir / name).chmod(0o755)
        clip = self.dir / "clip.oaf"
        clip.write_bytes(b"x")
        env = dict(os.environ, PATH=f"{bindir}:{os.environ['PATH']}", RUSTFLAGS="-C target-cpu=native",
                   CARGO_ENCODED_RUSTFLAGS="-Ctarget-cpu=native", REPLAY_COMPARE_GUARD="")
        out = self.dir / "out"
        res = subprocess.run(
            ["bash", str(SCRIPT), "HEAD", "HEAD", str(clip), str(out), str(self.dir / "scratch")],
            capture_output=True, text=True, env=env, timeout=120,
        )
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("build of", res.stderr)
        calls = log.read_text()
        build = [l for l in calls.splitlines() if l.startswith("cargo")][0]
        self.assertIn("--locked", build)
        self.assertIn("RUSTFLAGS=[unset]", build)
        self.assertIn("ENC=[unset]", build)


if __name__ == "__main__":
    unittest.main()
