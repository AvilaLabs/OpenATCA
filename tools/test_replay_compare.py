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


def json_q(text):
    """A shell single-quoted string of `text`."""
    return "'" + text.replace("'", "'\\''") + "'"


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
        # The preimage commit must be in the history this test runs in: a missing commit is a
        # failure (a shallow checkout would otherwise pass this test by skipping it).
        have = lambda: subprocess.run(
            ["git", "cat-file", "-e", "e251fdb^{commit}"], cwd=REPO, capture_output=True
        ).returncode == 0
        if not have():
            subprocess.run(["git", "fetch", "--quiet", "origin", "e251fdb"], cwd=REPO, capture_output=True)
        self.assertTrue(have(), "commit e251fdb (the overlay's preimage) is not in this clone's history")
        wt = self.dir / "wt"
        git(REPO, "worktree", "add", "--detach", str(wt), "e251fdb")
        try:
            res = self.apply(wt, OVERLAY_PATCH, "e251fdb")
            self.assertEqual(res.returncode, 0, res.stderr)
            self.assertIn("canon_spill.rs", (self.dir / "rep.txt").read_text())
        finally:
            git(REPO, "worktree", "remove", "--force", str(wt))
        head = self.dir / "head"
        git(REPO, "worktree", "add", "--detach", str(head), "HEAD")
        try:
            res = self.apply(head, OVERLAY_PATCH, "HEAD")
            self.assertEqual(res.returncode, 2)
        finally:
            git(REPO, "worktree", "remove", "--force", str(head))

    # REQ: OA-RDF-014
    def test_a_wrong_postimage_a_deleted_file_a_file_count_mismatch_and_a_missing_report_dir_are_refused(self):
        base = "one\ntwo\nthree\nfour\nfive\n"
        r = self.repo_with(base)
        p = self.patch_for(r)
        text = p.read_text()
        # Postimage: change the second blob id of the index line.
        m = re.search(r"^index ([0-9a-f]+)\.\.([0-9a-f]+)", text, re.M)
        bad = text.replace(m.group(0), f"index {m.group(1)}..{'0' * 3}{m.group(2)[3:]}")
        bp = self.dir / "bad_post.patch"
        bp.write_text(bad)
        res = self.apply(r, bp)
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("postimage mismatch", res.stderr)
        git(r, "checkout", "-q", "--", "f.txt")
        # A patch that deletes a file.
        (r / "f.txt").unlink()
        dele = git(r, "diff", "--no-color")
        git(r, "checkout", "-q", "--", "f.txt")
        dp = self.dir / "del.patch"
        dp.write_text(dele)
        res = self.apply(r, dp)
        self.assertEqual(res.returncode, 2)
        self.assertIn("deletes", res.stderr)
        # More files than index lines: a mode change.
        os.chmod(r / "f.txt", 0o755)
        mode = git(r, "diff", "--no-color")
        os.chmod(r / "f.txt", 0o644)
        mp = self.dir / "mode.patch"
        mp.write_text(mode)
        res = self.apply(r, mp)
        self.assertEqual(res.returncode, 2)
        self.assertTrue("index lines" in res.stderr or "names no file blobs" in res.stderr, res.stderr)
        # A report directory that does not exist is refused before anything is applied.
        res = subprocess.run(
            ["bash", str(OVERLAY_SCRIPT), str(r), "HEAD", str(p), str(self.dir / "nodir" / "rep.txt")],
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(res.returncode, 2)
        self.assertIn("report directory", res.stderr)
        self.assertEqual((r / "f.txt").read_text(), base)

    # REQ: OA-RDF-015
    def test_the_overlay_carries_the_trees_canonical_code_byte_for_byte(self):
        # The overlay creates canon.rs and canon_spill.rs; they must be the files of this tree,
        # which are what writes the digest line, the hash of a second and the chain.
        sections = re.split(r"(?m)^diff --git ", OVERLAY_PATCH.read_text())
        for name in ["canon.rs", "canon_spill.rs"]:
            sec = [x for x in sections if x.startswith(f"a/crates/oa-track/src/{name} ")]
            self.assertEqual(len(sec), 1, name)
            lines = [
                l[1:]
                for l in sec[0].splitlines()
                if l.startswith("+") and not l.startswith("+++")
            ]
            now = (REPO / "crates/oa-track/src" / name).read_text().splitlines()
            self.assertEqual(lines, now, f"{name} in the overlay differs from the tree")

    # REQ: OA-RDF-016
    def test_builds_run_locked_in_a_clean_environment_and_the_report_records_it(self):
        bindir = self.dir / "bin"
        bindir.mkdir()
        log = self.dir / "calls.log"
        tgt = self.dir / "tgt"
        run_stub = (
            "#!/usr/bin/env bash\n"
            'while [ $# -gt 0 ]; do case "$1" in --digest) echo digest-stub >"$2";; --rows) echo rows-stub >"$2";; esac; shift; done\n'
        )
        diff_stub = (
            "#!/usr/bin/env bash\n"
            'while [ $# -gt 0 ]; do case "$1" in --summary) echo identical=true >"$2";; esac; shift; done\n'
            "echo 'result: identical'\n"
        )
        cargo = (
            "#!/usr/bin/env bash\n"
            f'{{ echo "CALL cargo $*"; env | sort | sed "s/^/ENV /"; }} >>{log}\n'
            'pkg=""; while [ $# -gt 0 ]; do [ "$1" = -p ] && pkg="$2"; shift; done\n'
            'mkdir -p "$CARGO_TARGET_DIR/release"\n'
            f'case "$pkg" in oa-track) printf %s {json_q(run_stub)} >"$CARGO_TARGET_DIR/release/oa-track-run"; chmod +x "$CARGO_TARGET_DIR/release/oa-track-run";;\n'
            f'oa-replay-diff) printf %s {json_q(diff_stub)} >"$CARGO_TARGET_DIR/release/oa-replay-diff"; chmod +x "$CARGO_TARGET_DIR/release/oa-replay-diff";; esac\n'
        )
        (bindir / "cargo").write_text(cargo)
        (bindir / "rustc").write_text('#!/usr/bin/env bash\necho "rustc 1.89.0 stub"\n')
        for n in ["cargo", "rustc"]:
            (bindir / n).chmod(0o755)
        clip = self.dir / "clip.oaf"
        clip.write_bytes(b"x")
        env = dict(
            os.environ,
            PATH=f"{bindir}:{os.environ['PATH']}",
            RUSTFLAGS="-C target-cpu=native",
            CARGO_ENCODED_RUSTFLAGS="-Ctarget-cpu=native",
            CARGO_BUILD_RUSTFLAGS="-Cdebuginfo=2",
            RUSTDOCFLAGS="--cfg x",
            AN_UNRELATED_TEST_VARIABLE="1",
            CARGO_TARGET_DIR=str(tgt),
            REPLAY_COMPARE_GUARD="",
        )
        out = self.dir / "out"
        res = subprocess.run(
            ["bash", str(SCRIPT), "HEAD", "HEAD", str(clip), str(out), str(self.dir / "scratch")],
            capture_output=True, text=True, env=env, timeout=180,
        )
        self.assertEqual(res.returncode, 0, res.stderr)
        calls = log.read_text()
        builds = [l for l in calls.splitlines() if l.startswith("CALL cargo")]
        self.assertEqual(len(builds), 3)  # A, B, and the diff tool
        for b in builds:
            self.assertIn("--locked", b)
            self.assertIn("--config build.rustflags=[]", b)
        names = {l[4:].split("=")[0] for l in calls.splitlines() if l.startswith("ENV ")}
        allowed = {"PATH", "HOME", "RUSTUP_HOME", "CARGO_HOME", "TMPDIR", "CARGO_BUILD_JOBS",
                   "CARGO_TARGET_DIR", "PWD", "SHLVL", "OLDPWD", "_"}
        self.assertLessEqual(names, allowed, names - allowed)
        report = (out / "report.txt").read_text()
        for v in ["RUSTFLAGS=[-C target-cpu=native]", "CARGO_ENCODED_RUSTFLAGS=[-Ctarget-cpu=native]",
                  "CARGO_BUILD_RUSTFLAGS=[-Cdebuginfo=2]", "RUSTDOCFLAGS=[--cfg x]",
                  "AN_UNRELATED_TEST_VARIABLE", "environment variables dropped:",
                  "rustc 1.89.0 stub", "diff tool:", "overlay for commit A: none"]:
            self.assertIn(v, report)
        self.assertEqual(len(re.findall(r"digest sha256: [0-9a-f]{64}", report)), 2)
        self.assertEqual(len(re.findall(r"rows sha256: [0-9a-f]{64}", report)), 2)
        self.assertEqual(len(re.findall(r"(?<!diff )binary sha256: [0-9a-f]{64}", report)), 2)
        self.assertRegex(report, r"diff binary sha256: [0-9a-f]{64}")
        summary = (out / "summary.txt").read_text()
        self.assertIn("rustflags_cleared=yes", summary)
        self.assertRegex(summary, r"digest_A=[0-9a-f]{64}")
        self.assertRegex(summary, r"rows_B=[0-9a-f]{64}")

    # REQ: OA-RDF-022
    def test_a_missing_diff_binary_is_exit_2(self):
        bindir = self.dir / "bin"
        bindir.mkdir()
        # A cargo that builds only the tracker: the diff binary is never produced.
        (bindir / "run_src").write_text(
            "#!/usr/bin/env bash\n"
            'while [ $# -gt 0 ]; do case "$1" in --digest) echo d >"$2";; --rows) echo r >"$2";; esac; shift; done\n'
        )
        (bindir / "cargo").write_text(
            "#!/usr/bin/env bash\n"
            'pkg=""; while [ $# -gt 0 ]; do [ "$1" = -p ] && pkg="$2"; shift; done\n'
            'mkdir -p "$CARGO_TARGET_DIR/release"\n'
            f'[ "$pkg" = oa-track ] && {{ cp {bindir}/run_src "$CARGO_TARGET_DIR/release/oa-track-run"; chmod +x "$CARGO_TARGET_DIR/release/oa-track-run"; }}\n'
            "exit 0\n"
        )
        (bindir / "rustc").write_text('#!/usr/bin/env bash\necho stub\n')
        for n in ["cargo", "rustc"]:
            (bindir / n).chmod(0o755)
        clip = self.dir / "clip.oaf"
        clip.write_bytes(b"x")
        env = dict(os.environ, PATH=f"{bindir}:{os.environ['PATH']}", REPLAY_COMPARE_GUARD="",
                   CARGO_TARGET_DIR=str(self.dir / "tgt"))
        res = subprocess.run(
            ["bash", str(SCRIPT), "HEAD", "HEAD", str(clip), str(self.dir / "out"), str(self.dir / "scratch")],
            capture_output=True, text=True, env=env, timeout=180,
        )
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("is not at", res.stderr)


if __name__ == "__main__":
    unittest.main()
