import os
import re
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_fuzz_shard as shard

REPO = Path(__file__).resolve().parents[2]


class FuzzShardTests(unittest.TestCase):
    def test_cargo_listing_matches_every_declared_target(self):
        targets = shard.cargo_bin_inventory(REPO / "fuzz" / "Cargo.toml")
        self.assertTrue(targets)
        self.assertEqual(len(targets), len(set(targets)))
        self.assertEqual(shard.cargo_fuzz_listing(("\n".join(targets) + "\n").encode(), targets), targets)

    def test_inventory_refuses_duplicate_missing_extra_empty_and_invalid_names(self):
        declared = ["adsb", "xml"]
        for payload in (b"adsb\nadsb\n", b"adsb\n", b"adsb\nxml\nother\n", b"", b"adsb\nnot a target\n"):
            with self.subTest(payload=payload), self.assertRaises(shard.FuzzShardError):
                shard.cargo_fuzz_listing(payload, declared)

    def test_five_shards_cover_all_23_targets_exactly_once_and_balance(self):
        targets = shard.cargo_bin_inventory(REPO / "fuzz" / "Cargo.toml")
        self.assertEqual(len(targets), 23)
        shards = [shard.targets_for_shard(targets, i) for i in range(5)]
        flattened = [target for group in shards for target in group]
        self.assertEqual(sorted(flattened), targets)
        self.assertEqual(len(flattened), len(set(flattened)))
        self.assertLessEqual(max(map(len, shards)) - min(map(len, shards)), 1)
        self.assertEqual(shards, [shard.targets_for_shard(targets, i) for i in range(5)])
        shard.validate_shard_union(targets)

    def test_shard_index_and_count_are_fixed(self):
        with self.assertRaises(shard.FuzzShardError):
            shard.targets_for_shard(["x"], 5)
        with self.assertRaises(shard.FuzzShardError):
            shard.targets_for_shard(["x"], 0, 3)

    def test_run_keeps_original_rss_and_time_limits_and_propagates_failure(self):
        targets = ["alpha"]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "fuzz").mkdir()
            runner_temp = root / "runner"
            runner_temp.mkdir()
            with patch.object(shard.subprocess, "run", return_value=type("Result", (), {"returncode": 7})()) as run:
                result = shard.run_shard(targets, 0, root, runner_temp)
        self.assertEqual(result, 7)
        argv = run.call_args.args[0]
        self.assertEqual(argv[:5], ["cargo", "+nightly-2026-08-21", "fuzz", "run", "alpha"])
        self.assertIn("-rss_limit_mb=2048", argv)
        self.assertIn("-max_total_time=60", argv)

    def test_seed_caps_cover_every_checked_in_seed(self):
        seeds_root = REPO / "fuzz" / "seeds"
        target_dirs = []
        with os.scandir(seeds_root) as scan:
            for entry in scan:
                self.assertLess(len(target_dirs), shard.MAX_TARGETS)
                info = entry.stat(follow_symlinks=False)
                if stat.S_ISDIR(info.st_mode):
                    target_dirs.append(Path(entry.path))
        target_dirs.sort(key=lambda path: path.name)
        total_files = 0
        for seed_dir in target_dirs:
            entries = []
            total_bytes = 0
            with os.scandir(seed_dir) as scan:
                for entry in scan:
                    self.assertLess(len(entries), shard.MAX_SEEDS_PER_TARGET, seed_dir.name)
                    info = entry.stat(follow_symlinks=False)
                    self.assertTrue(stat.S_ISREG(info.st_mode), entry.name)
                    self.assertLessEqual(info.st_size, shard.MAX_SEED_FILE_BYTES, entry.name)
                    total_bytes += info.st_size
                    entries.append(entry.name)
            self.assertLessEqual(total_bytes, shard.MAX_SEED_BYTES_PER_TARGET, seed_dir.name)
            total_files += len(entries)
        self.assertLessEqual(total_files, shard.MAX_TARGETS * shard.MAX_SEEDS_PER_TARGET)

    def test_bounded_seed_copy_preserves_every_file_byte(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            seeds = root / "seeds"
            corpus = root / "corpus"
            seeds.mkdir()
            corpus.mkdir()
            samples = {"one": b"\x00seed-one", "two": b"seed-two\xff"}
            for name, payload in samples.items():
                (seeds / name).write_bytes(payload)
            self.assertEqual(shard.copy_seeds_bounded(seeds, corpus), len(samples))
            self.assertEqual({p.name: p.read_bytes() for p in corpus.iterdir()}, samples)

    def test_seed_count_file_and_total_byte_caps_refuse_before_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            seeds = root / "seeds"
            corpus = root / "corpus"
            seeds.mkdir()
            corpus.mkdir()
            for index in range(shard.MAX_SEEDS_PER_TARGET + 1):
                (seeds / f"s{index:03}").write_bytes(b"")
            with self.assertRaises(shard.FuzzShardError):
                shard.copy_seeds_bounded(seeds, corpus)
            self.assertEqual(list(corpus.iterdir()), [])

        for kind in ("single", "aggregate"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                seeds = root / "seeds"
                corpus = root / "corpus"
                seeds.mkdir()
                corpus.mkdir()
                count = 1 if kind == "single" else shard.MAX_SEED_BYTES_PER_TARGET // shard.MAX_SEED_FILE_BYTES + 1
                size = shard.MAX_SEED_FILE_BYTES + 1 if kind == "single" else shard.MAX_SEED_FILE_BYTES
                for index in range(count):
                    (seeds / f"s{index:03}").write_bytes(b"x" * size)
                with self.assertRaises(shard.FuzzShardError):
                    shard.copy_seeds_bounded(seeds, corpus)
                self.assertEqual(list(corpus.iterdir()), [])

    def test_bounded_input_refuses_nonregular_paths_before_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(shard.FuzzShardError):
                shard._bounded_bytes(Path(tmp), 1024, "test directory")

    def test_ci_uses_checked_enumeration_five_shards_cache_and_unique_artifacts(self):
        workflow = (REPO / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        start = workflow.index("  fuzz-smoke:")
        boundary = re.search(r"\n  [a-z][a-z0-9-]*:\n", workflow[start + 1 :])
        end = len(workflow) if boundary is None else start + 1 + boundary.start()
        block = workflow[start:end]
        self.assertIn("fail-fast: false", block)
        self.assertIn("shard: [0, 1, 2, 3, 4]", block)
        self.assertIn("set -euo pipefail", block)
        self.assertIn("fuzz list > \"$RUNNER_TEMP/fuzz-targets.txt\"", block)
        self.assertIn('--shard "${{ matrix.shard }}"', block)
        self.assertIn("fuzz-artifacts-shard-${{ matrix.shard }}", block)
        self.assertIn("actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9", block)
        self.assertIn("${{ runner.arch }}", block)
        self.assertIn("${{ matrix.shard }}", block)
        self.assertIn("${{ runner.os }}", block)
        self.assertIn("${{ hashFiles('fuzz/Cargo.lock') }}", block)
        self.assertIn("fuzz-smoke", block)
        self.assertIn("~/.cargo/registry", block)
        self.assertIn("~/.cargo/git", block)
        self.assertIn("fuzz/target", block)
        self.assertIn("cache_hit=%s", block)
        self.assertIn("target_seconds=60", block)
        self.assertIn("Remove stale cached example executables", block)
        runner = (REPO / ".github" / "scripts" / "run_fuzz_shard.py").read_text()
        self.assertIn("MAX_TOTAL_TIME_SECONDS = 60", runner)
        self.assertIn("RSS_LIMIT_MB = 2048", runner)
        self.assertIn('-max_total_time={MAX_TOTAL_TIME_SECONDS}', runner)
        self.assertIn('validate_shard_union(targets)', runner)


if __name__ == "__main__":
    unittest.main()
