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
CLEAN_ENV = Path(__file__).resolve().parent / "replay_clean_env.sh"
REPO = Path(__file__).resolve().parent.parent


def run(*args, cwd=None, script=SCRIPT):
    return subprocess.run(
        ["bash", str(script), *args],
        capture_output=True,
        text=True,
        cwd=cwd,
        timeout=60,
    )


# The files the script needs beside it (the sealed-data check reads the list from the crate).
SCRIPT_FILES = ["replay_compare.sh", "replay_clean_env.sh", "replay_overlay_apply.sh", "sealed_guard.py"]


# A stand-in for oa-clip-sealcheck that finds nothing in the window.
SEAL_OK_STUB = "#!/usr/bin/env bash\necho 'clip: 1 frames, 1 rows; in the sealed window: 0'\nexit 0\n"


def scratch_repo(base, commits):
    """A private repository holding copies of the script and one commit per (name, canon, main).

    The tests that name commits use this, never the shared repository: the script adds and prunes
    worktrees of the repository it sits in, which other sessions are using. Nothing here reaches
    the network.
    """
    repo = Path(base) / "r"
    (repo / "tools").mkdir(parents=True)
    (repo / "crates/oa-track/src").mkdir(parents=True)
    (repo / "crates/oa-sealed").mkdir(parents=True)
    for n in SCRIPT_FILES:
        (repo / "tools" / n).write_text((SCRIPT.parent / n).read_text())
    (repo / "crates/oa-sealed/sealed_paths.txt").write_text(
        (REPO / "crates/oa-sealed/sealed_paths.txt").read_text()
    )
    git(repo, "init", "-q", "-b", "main")
    out = {}
    for name, canon, main in commits:
        (repo / "crates/oa-track/src/canon.rs").write_text(canon)
        (repo / "crates/oa-track/src/main.rs").write_text(main)
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", name)
        out[name] = git(repo, "rev-parse", "HEAD").strip()
    return repo, out


class ReplayCompareArguments(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        # A private repository: `HEAD` is its one commit, which has the v2 digest format.
        self.repo, _ = scratch_repo(self.dir, [("v2", "// oa-track digest v2: header\n", 'x "--digest" y\n')])
        self.clip = self.dir / "clip.oaf"
        self.clip.write_bytes(b"x")
        self.out = self.dir / "out"

    def tearDown(self):
        self.tmp.cleanup()

    def script_run(self, *args):
        return run(*args, script=self.repo / "tools/replay_compare.sh", cwd=self.repo)

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
                r = self.script_run(bad, "HEAD", str(self.clip), str(self.out))
                self.assertEqual(r.returncode, 2, r.stderr)
                self.assertIn("not a commit hash or ref", r.stderr)
                self.assertFalse((self.out / "work").exists())
        for opt in ["--help", "-x"]:
            r = self.script_run(opt, "HEAD", str(self.clip), str(self.out))
            self.assertEqual(r.returncode, 2)
            self.assertIn("usage", r.stderr)
        r = self.script_run("HEAD", "no-such-ref-for-the-test", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such commit", r.stderr)

    # REQ: OA-RDF-006
    def test_missing_clip_wrong_argument_count_and_existing_report_are_refused(self):
        r = self.script_run("HEAD", "HEAD", str(self.dir / "nope.oaf"), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such clip", r.stderr)
        r = self.script_run("HEAD", "HEAD")
        self.assertEqual(r.returncode, 2)
        self.assertIn("usage", r.stderr)
        r = self.script_run("HEAD", "HEAD", str(self.clip), str(self.out), "--expect", "missing.txt")
        self.assertEqual(r.returncode, 2)
        self.assertIn("no such expect file", r.stderr)
        self.out.mkdir()
        (self.out / "report.txt").write_text("old")
        r = self.script_run("HEAD", "HEAD", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2)
        self.assertIn("exists", r.stderr)
        self.assertEqual((self.out / "report.txt").read_text(), "old")

    # REQ: OA-RDF-023
    def test_a_non_empty_output_directory_is_refused_before_anything_is_built(self):
        self.out.mkdir()
        for name in ["overlay-A.txt", "hashes-A.txt", "time-A.txt", "rows-A.bin"]:
            (self.out / name).write_text("stale")
        r = self.script_run("HEAD", "HEAD", str(self.clip), str(self.out))
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("not empty", r.stderr)
        # Nothing was built or removed: the stale files are exactly as they were.
        self.assertEqual(sorted(p.name for p in self.out.iterdir()),
                         ["hashes-A.txt", "overlay-A.txt", "rows-A.bin", "time-A.txt"])
        self.assertEqual((self.out / "rows-A.bin").read_text(), "stale")
        # An empty existing directory is fine (it gets as far as the build, which is refused here
        # for the clip, not for the directory).
        empty = self.dir / "empty"
        empty.mkdir()
        r = self.script_run("HEAD", "HEAD", str(self.dir / "nope.oaf"), str(empty))
        self.assertIn("no such clip", r.stderr)


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
            f'env | sort >>{log}.run\n'
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
            f'oa-replay-diff) printf %s {json_q(diff_stub)} >"$CARGO_TARGET_DIR/release/oa-replay-diff"; chmod +x "$CARGO_TARGET_DIR/release/oa-replay-diff";;\n'
            f'oa-truth) printf %s {json_q(SEAL_OK_STUB)} >"$CARGO_TARGET_DIR/release/oa-clip-sealcheck"; chmod +x "$CARGO_TARGET_DIR/release/oa-clip-sealcheck";; esac\n'
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
        self.assertEqual(len(builds), 4)  # the clip check, A, B, and the diff tool
        for b in builds:
            self.assertIn("--locked", b)
            self.assertIn("--config build.rustflags=[]", b)
        names = {l[4:].split("=")[0] for l in calls.splitlines() if l.startswith("ENV ")}
        allowed = {"PATH", "HOME", "RUSTUP_HOME", "CARGO_HOME", "TMPDIR", "CARGO_BUILD_JOBS",
                   "CARGO_TARGET_DIR", "CARGO_ENCODED_RUSTFLAGS", "PWD", "SHLVL", "OLDPWD", "_"}
        self.assertLessEqual(names, allowed, names - allowed)
        run_env = {l.split("=")[0] for l in Path(f"{log}.run").read_text().splitlines()}
        self.assertLessEqual(run_env, allowed, run_env - allowed)
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
        self.assertRegex(summary, r"diff_tool_commit=[0-9a-f]{40}")
        self.assertRegex(summary, r"diff_tool_tree_clean=(yes|no)")
        self.assertIn("digest_format_A=v2", summary)
        self.assertIn("diff tool build commit:", report)
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
            f'[ "$pkg" = oa-truth ] && {{ printf %s {json_q(SEAL_OK_STUB)} >"$CARGO_TARGET_DIR/release/oa-clip-sealcheck"; chmod +x "$CARGO_TARGET_DIR/release/oa-clip-sealcheck"; }}\n'
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
        # A failed run leaves no rows file behind (OA-RDF-023).
        self.assertEqual(sorted(p.name for p in (self.dir / "out").glob("rows-*")), [])


    # REQ: OA-SEAL-014
    def test_a_clip_that_fails_the_seal_check_refuses_the_run_before_any_commit_is_built(self):
        for code in (1, 2):
            with self.subTest(exit=code):
                d = self.dir / f"c{code}"
                bindir = d / "bin"
                bindir.mkdir(parents=True)
                log = d / "calls.log"
                seal = f"#!/usr/bin/env bash\necho 'in the sealed window: 1 rows'\nexit {code}\n"
                (bindir / "cargo").write_text(
                    "#!/usr/bin/env bash\n"
                    f'echo "$*" >>{log}\n'
                    'pkg=""; while [ $# -gt 0 ]; do [ "$1" = -p ] && pkg="$2"; shift; done\n'
                    'mkdir -p "$CARGO_TARGET_DIR/release"\n'
                    f'[ "$pkg" = oa-truth ] && {{ printf %s {json_q(seal)} >"$CARGO_TARGET_DIR/release/oa-clip-sealcheck"; chmod +x "$CARGO_TARGET_DIR/release/oa-clip-sealcheck"; }}\n'
                    "exit 0\n"
                )
                (bindir / "rustc").write_text("#!/usr/bin/env bash\necho stub\n")
                for n in ["cargo", "rustc"]:
                    (bindir / n).chmod(0o755)
                clip = d / "clip.oaf"
                clip.write_bytes(b"x")
                env = dict(os.environ, PATH=f"{bindir}:{os.environ['PATH']}", REPLAY_COMPARE_GUARD="",
                           CARGO_TARGET_DIR=str(d / "tgt"))
                res = subprocess.run(
                    ["bash", str(SCRIPT), "HEAD", "HEAD", str(clip), str(d / "out"), str(d / "scratch")],
                    capture_output=True, text=True, env=env, timeout=180,
                )
                self.assertEqual(res.returncode, 2, res.stderr)
                self.assertIn("failed the seal check", res.stderr)
                # Only the clip check was built: no commit was built or run.
                calls = log.read_text().splitlines()
                self.assertEqual(len(calls), 1, calls)
                self.assertIn("oa-clip-sealcheck", calls[0])
                self.assertEqual(sorted(p.name for p in (d / "out").glob("digest-*")), [])


    def stubbed(self, d, seal, run_stub, fail_seal_build=False):
        """A directory `d` with stub cargo and rustc: the oa-truth build makes `seal`, the oa-track
        build makes `run_stub`, and the diff build a stub that finds nothing. Returns (env, clip)."""
        bindir = d / "bin"
        bindir.mkdir(parents=True)
        diff_stub = (
            "#!/usr/bin/env bash\n"
            'while [ $# -gt 0 ]; do case "$1" in --summary) echo identical=true >"$2";; esac; shift; done\n'
            "echo 'result: identical'\n"
        )
        build_seal = (
            "exit 1\n"
            if fail_seal_build
            else f'printf %s {json_q(seal)} >"$CARGO_TARGET_DIR/release/oa-clip-sealcheck"; chmod +x "$CARGO_TARGET_DIR/release/oa-clip-sealcheck"\n'
        )
        (bindir / "cargo").write_text(
            "#!/usr/bin/env bash\n"
            f'echo "$*" >>{d}/cargo.log\n'
            'pkg=""; while [ $# -gt 0 ]; do [ "$1" = -p ] && pkg="$2"; shift; done\n'
            'mkdir -p "$CARGO_TARGET_DIR/release"\n'
            f'case "$pkg" in oa-truth) {build_seal};;\n'
            f'oa-track) printf %s {json_q(run_stub)} >"$CARGO_TARGET_DIR/release/oa-track-run"; chmod +x "$CARGO_TARGET_DIR/release/oa-track-run";;\n'
            f'oa-replay-diff) printf %s {json_q(diff_stub)} >"$CARGO_TARGET_DIR/release/oa-replay-diff"; chmod +x "$CARGO_TARGET_DIR/release/oa-replay-diff";; esac\n'
        )
        (bindir / "rustc").write_text("#!/usr/bin/env bash\necho stub\n")
        for n in ["cargo", "rustc"]:
            (bindir / n).chmod(0o755)
        clip = d / "clip.oaf"
        clip.write_bytes(b"original-bytes")
        env = dict(os.environ, PATH=f"{bindir}:{os.environ['PATH']}", REPLAY_COMPARE_GUARD="",
                   CARGO_TARGET_DIR=str(d / "tgt"))
        return env, clip

    def go(self, d, env, clip):
        return subprocess.run(
            ["bash", str(SCRIPT), "HEAD", "HEAD", str(clip), str(d / "out"), str(d / "scratch")],
            capture_output=True, text=True, env=env, timeout=180,
        )

    # REQ: OA-SEAL-014
    def test_the_check_and_both_runs_read_one_private_read_only_copy_of_the_clip(self):
        d = self.dir / "copy"
        orig = d / "clip.oaf"
        seal = (
            "#!/usr/bin/env bash\n"
            f'echo "$1" >{d}/seal.arg\n'
            f'stat -c %a "$1" >{d}/seal.mode; stat -c %a "$(dirname "$1")" >{d}/seal.dirmode\n'
            # The original is swapped after the check, as an attacker or another job could.
            f'rm -f {orig}; printf SWAPPED >{orig}\n'
            "exit 0\n"
        )
        run_stub = (
            "#!/usr/bin/env bash\n"
            f'while [ $# -gt 0 ]; do case "$1" in --adsb) echo "$2" >>{d}/run.arg; cat "$2" >>{d}/run.bytes;; '
            '--digest) echo d >"$2";; --rows) echo r >"$2";; esac; shift; done\n'
        )
        env, clip = self.stubbed(d, seal, run_stub)
        res = self.go(d, env, clip)
        self.assertEqual(res.returncode, 0, res.stderr)
        seal_arg = (d / "seal.arg").read_text().strip()
        run_args = (d / "run.arg").read_text().split()
        # The check and both runs were given the same path, which is not the original's.
        self.assertEqual(run_args, [seal_arg, seal_arg])
        self.assertNotEqual(seal_arg, str(clip))
        self.assertTrue(seal_arg.startswith(str(d / "scratch")), seal_arg)
        # They saw the bytes that were checked, though the original was swapped after the check.
        self.assertEqual((d / "run.bytes").read_bytes(), b"original-bytes" * 2)
        self.assertEqual(clip.read_bytes(), b"SWAPPED")
        self.assertEqual((d / "seal.mode").read_text().strip(), "400")
        self.assertEqual((d / "seal.dirmode").read_text().strip(), "700")
        import hashlib
        h = hashlib.sha256(b"original-bytes").hexdigest()
        self.assertIn(f"clip_sha256={h}", (d / "out" / "summary.txt").read_text())

    # REQ: OA-SEAL-014
    def test_a_clip_copy_that_changes_during_the_runs_fails_the_run(self):
        d = self.dir / "mutate"
        run_stub = (
            "#!/usr/bin/env bash\n"
            'while [ $# -gt 0 ]; do case "$1" in --adsb) chmod u+w "$2"; echo more >>"$2";; '
            '--digest) echo d >"$2";; --rows) echo r >"$2";; esac; shift; done\n'
        )
        env, clip = self.stubbed(d, SEAL_OK_STUB, run_stub)
        res = self.go(d, env, clip)
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("changed during the runs", res.stderr)
        self.assertFalse((d / "out" / "report.txt").exists())

    # REQ: OA-SEAL-014
    def test_a_failed_build_of_the_seal_check_refuses_the_run(self):
        d = self.dir / "nobuild"
        env, clip = self.stubbed(d, SEAL_OK_STUB, "#!/usr/bin/env bash\nexit 0\n", fail_seal_build=True)
        res = self.go(d, env, clip)
        self.assertEqual(res.returncode, 2, res.stderr)
        self.assertIn("build of oa-clip-sealcheck failed", res.stderr)
        self.assertEqual(len((d / "cargo.log").read_text().splitlines()), 1)
        self.assertEqual(sorted(p.name for p in (d / "out").glob("digest-*")), [])


    # REQ: OA-SEAL-014
    def test_a_relative_cargo_target_dir_is_resolved_before_the_builds_change_directory(self):
        d = self.dir / "reltgt"
        run_stub = (
            "#!/usr/bin/env bash\n"
            'while [ $# -gt 0 ]; do case "$1" in --digest) echo d >"$2";; --rows) echo r >"$2";; esac; shift; done\n'
        )
        env, clip = self.stubbed(d, SEAL_OK_STUB, run_stub)
        env["CARGO_TARGET_DIR"] = "reltgt-dir"
        res = subprocess.run(
            ["bash", str(SCRIPT), "HEAD", "HEAD", str(clip), str(d / "out"), str(d / "scratch")],
            capture_output=True, text=True, env=env, timeout=180, cwd=d,
        )
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertTrue((d / "reltgt-dir/release/oa-clip-sealcheck").exists())


class DigestFormatRefusal(unittest.TestCase):
    """A scratch repository with the script, so commits of any digest format can be named."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        self.repo, self.commits = scratch_repo(self.d, [
            ("v1", "// oa-track digest v1: header\n", 'x "--digest" y\n'),
            ("v2", "// oa-track digest v2: header\n", 'x "--digest" y\n'),
            ("half", "// oa-track digest v2: header\n", "no flag\n"),
            ("odd", "// some other header\n", 'x "--digest" y\n'),
            # The flag is on the first line of a file far longer than a pipe buffer: a reader
            # that stops at the first match (grep -q) used to make `git show` die of SIGPIPE.
            ("big", "// oa-track digest v2: header\n", '"--digest"\n' + ("// padding\n" * 200_000)),
        ])
        self.clip = self.d / "clip.oaf"
        self.clip.write_bytes(b"x")

    def tearDown(self):
        self.tmp.cleanup()

    def go(self, a, b):
        out = self.d / "out"
        r = subprocess.run(
            ["bash", str(self.repo / "tools/replay_compare.sh"), self.commits[a], self.commits[b],
             str(self.clip), str(out)],
            capture_output=True, text=True, timeout=60,
            env=dict(os.environ, REPLAY_COMPARE_GUARD=""),
        )
        return r, out

    # REQ: OA-RDF-024
    def test_a_commit_that_would_emit_digest_v1_or_a_half_way_commit_is_refused_before_a_build(self):
        for a, b, msg in [("v1", "v2", "would emit digest v1"), ("v2", "v1", "would emit digest v1"),
                          ("half", "v2", "half-way"), ("v2", "odd", "does not know")]:
            with self.subTest(a=a, b=b):
                r, out = self.go(a, b)
                self.assertEqual(r.returncode, 2, r.stderr)
                self.assertIn(msg, r.stderr)
                self.assertFalse((out / "work").exists())
                self.assertFalse(any("building" in l for l in r.stderr.splitlines()))

    # REQ: OA-RDF-024
    def test_a_commit_with_the_flag_in_a_large_file_is_not_read_as_half_way(self):
        # Repeated: the failure was a race, so it showed in one run of many.
        for _ in range(20):
            r, out = self.go("big", "big")
            self.assertNotIn("half-way", r.stderr)
            self.assertNotIn("digest format this script does not know", r.stderr)


class CleanBuildEnvironment(unittest.TestCase):
    # REQ: OA-RDF-016
    def test_a_target_rustflags_config_in_cargo_home_does_not_reach_the_compiler(self):
        home = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup"))
        if not (home / "toolchains").is_dir():
            self.skipTest("no rustup home")
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            (t / "cargo_home").mkdir()
            (t / "cargo_home" / "config.toml").write_text(
                '[target.x86_64-unknown-linux-gnu]\nrustflags = ["-C", "target-cpu=native"]\n'
            )
            crate = t / "c"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2021"\n'
            )
            (crate / "src" / "lib.rs").write_text("")
            base = dict(os.environ, RUSTUP_HOME=str(home), CARGO_HOME=str(t / "cargo_home"))
            # The config does reach a build run the plain way (so the check below proves something).
            plain = subprocess.run(
                ["env", "-i", f"PATH={base['PATH']}", f"HOME={t}", f"RUSTUP_HOME={home}",
                 f"CARGO_HOME={t / 'cargo_home'}", "cargo", "+1.89.0", "build", "-v", "--offline",
                 "--config", "build.rustflags=[]", "--target-dir", str(t / "tp")],
                cwd=crate, capture_output=True, text=True, timeout=120,
            )
            self.assertIn("target-cpu=native", plain.stderr, plain.stderr)
            # The script's environment ignores it.
            res = subprocess.run(
                ["bash", "-c",
                 '. "$1"; replay_clean_env; "${clean_env_prefix[@]}" cargo +1.89.0 build -v --offline '
                 "--config 'build.rustflags=[]' --target-dir \"$2\"",
                 "x", str(CLEAN_ENV), str(t / "tc")],
                cwd=crate, capture_output=True, text=True, timeout=120, env=base,
            )
            self.assertEqual(res.returncode, 0, res.stderr)
            self.assertIn("rustc --crate-name probe", res.stderr)
            self.assertNotIn("target-cpu", res.stderr)
            self.assertNotIn("-C target", res.stderr)


if __name__ == "__main__":
    unittest.main()
