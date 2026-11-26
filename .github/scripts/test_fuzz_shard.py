import re
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

    def test_four_shards_cover_each_target_exactly_once_and_balance(self):
        targets = shard.cargo_bin_inventory(REPO / "fuzz" / "Cargo.toml")
        shards = [shard.targets_for_shard(targets, i) for i in range(4)]
        flattened = [target for group in shards for target in group]
        self.assertEqual(sorted(flattened), targets)
        self.assertEqual(len(flattened), len(set(flattened)))
        self.assertLessEqual(max(map(len, shards)) - min(map(len, shards)), 1)
        self.assertEqual(shards, [shard.targets_for_shard(targets, i) for i in range(4)])

    def test_shard_index_and_count_are_fixed(self):
        with self.assertRaises(shard.FuzzShardError):
            shard.targets_for_shard(["x"], 4)
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

    def test_ci_uses_checked_enumeration_four_shards_and_unique_artifact_names(self):
        workflow = (REPO / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        start = workflow.index("  fuzz-smoke:")
        boundary = re.search(r"\n  [a-z][a-z0-9-]*:\n", workflow[start + 1 :])
        end = len(workflow) if boundary is None else start + 1 + boundary.start()
        block = workflow[start:end]
        self.assertIn("fail-fast: false", block)
        self.assertIn("shard: [0, 1, 2, 3]", block)
        self.assertIn("set -euo pipefail", block)
        self.assertIn("fuzz list > \"$RUNNER_TEMP/fuzz-targets.txt\"", block)
        self.assertIn('--shard "${{ matrix.shard }}"', block)
        self.assertIn("fuzz-artifacts-shard-${{ matrix.shard }}", block)
        runner = (REPO / ".github" / "scripts" / "run_fuzz_shard.py").read_text()
        self.assertIn("MAX_TOTAL_TIME_SECONDS = 60", runner)
        self.assertIn("RSS_LIMIT_MB = 2048", runner)
        self.assertIn('-max_total_time={MAX_TOTAL_TIME_SECONDS}', runner)


if __name__ == "__main__":
    unittest.main()
