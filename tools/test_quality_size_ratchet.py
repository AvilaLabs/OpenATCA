import unittest
import subprocess
import tempfile
from pathlib import Path
from unittest.mock import patch

from tools import quality_size_ratchet as ratchet


def repeated_block(prefix: str) -> str:
    return "\n".join([
        f"let {prefix}_alpha = source.alpha();",
        f"let {prefix}_bravo = source.bravo();",
        f"let {prefix}_charlie = source.charlie();",
        f"let {prefix}_delta = source.delta();",
        f"let {prefix}_echo = source.echo();",
        f"let {prefix}_foxtrot = source.foxtrot();",
        f"let {prefix}_golf = source.golf();",
        f"let {prefix}_hotel = source.hotel();",
        f"let {prefix}_india = source.india();",
        f"let {prefix}_juliet = source.juliet();",
    ])


class QualitySizeRatchetTests(unittest.TestCase):
    def assert_single_function(self, path, source, name, argument_count):
        functions = ratchet._function_tokens(path, source)
        self.assertEqual(len(functions), 1)
        self.assertEqual(functions[0].name, name)
        self.assertEqual(functions[0].argument_count, argument_count)
    def baseline_for(self, summary):
        return {
            "schema": 1,
            "algorithm": ratchet.ALGORITHM,
            "scope": ratchet.SCOPE,
            "workspace_binding": ratchet.WORKSPACE_BINDING,
            "limits": {
                "long_function_lines_exceeds": 150,
                "argument_count_exceeds": 7,
                "duplicate_window_tokens": ratchet.WINDOW_TOKENS,
                "duplicate_minimum_distinct_identifiers": ratchet.MIN_IDENTIFIERS,
                "tracked_source_files_max": ratchet.MAX_SOURCE_FILES,
                "tracked_source_bytes_max": ratchet.MAX_SOURCE_BYTES,
                "tracked_source_file_bytes_max": ratchet.MAX_SOURCE_FILE_BYTES,
                "baseline_bytes_max": ratchet.MAX_BASELINE_BYTES,
                "function_body_tokens_max": ratchet.MAX_CODE_TOKENS,
                "duplicate_windows_max": ratchet.MAX_DUPLICATE_WINDOWS,
                "duplicate_block_groups_max": ratchet.MAX_DUPLICATE_BLOCK_GROUPS,
                "duplicate_comparisons_max": ratchet.MAX_DUPLICATE_COMPARISONS,
                "lexer_tokens_per_file_max": ratchet.MAX_LEX_TOKENS_PER_FILE,
            },
            "seed_revision": "a" * 40,
            "summary": summary,
        }

    def test_rust_multiline_generics_and_comments_count_arguments(self):
        source = """pub(crate) fn bounded<T: Trait<Left, Right>>(
    first: T, // comments and nested type commas don't add arguments
    second: Vec<Vec<u8>>,
    third: usize,
    fourth: i32,
    fifth: i32,
    sixth: i32,
    seventh: i32,
    eighth: i32,
) -> Result<T, Error> { Ok(first) }
"""
        self.assert_single_function("sample.rs", source, "bounded", 8)

    def test_rust_trait_declarations_are_counted_without_a_body(self):
        source = "trait Wide { fn inspect(a,b,c,d,e,f,g,h); }\n"
        self.assert_single_function("sample.rs", source, "inspect", 8)

    def test_python_multiline_parameters_and_async_functions_are_counted(self):
        source = """async def bounded(
    first,
    second: tuple[int, str],
    third,
    fourth,
    fifth,
    sixth,
    seventh,
    eighth,
):
    return first
"""
        self.assert_single_function("sample.py", source, "bounded", 8)

    def test_function_line_limit_counts_physical_lines_including_comments(self):
        source = "fn long() {\n" + "\n".join("    // retained line" for _ in range(149)) + "\n}\n"
        summary = ratchet.analyze_sources([("sample.rs", source)])
        self.assertEqual(summary["long_function_count"], 1)
        self.assertEqual(summary["long_functions"][0]["lines"], 151)

    def test_duplicate_nested_function_body_is_not_counted_again_at_same_location(self):
        body = repeated_block("nested")
        source = "fn outer(source: Source) {\n    fn inner(source: Source) {\n" + body + "\n    }\n}\n"
        summary = ratchet.analyze_sources([("nested.rs", source)])
        self.assertEqual(summary["duplicate_block_count"], 0)

    def test_window_limit_is_derived_from_total_body_token_limit(self):
        functions = ratchet._function_tokens("window.rs", "fn sample() {\n" + repeated_block("x") + "\n}\n")
        ids, identifiers = ratchet._intern_function_bodies(functions)
        windows = list(ratchet._qualified_windows(ids, identifiers))
        self.assertLessEqual(len(windows), sum(len(body) for body in ids))
        self.assertEqual(ratchet.MAX_DUPLICATE_WINDOWS, ratchet.MAX_CODE_TOKENS)

    def test_packed_location_rejects_overflow_and_accepts_boundary(self):
        largest = ratchet.PACKED_LOCATION_PART_MAX
        self.assertEqual(ratchet._encode_location(largest, largest), (largest << 32) | largest)
        with self.assertRaises(ratchet.RatchetError):
            ratchet._encode_location(largest + 1, 0)

    def test_hash_collision_does_not_create_false_duplicate(self):
        body = repeated_block("stable")
        functions = ratchet._function_tokens(
            "collision.rs",
            "fn first(source: Source) {\n" + body + "\n}\n"
            "fn second(source: Source) {\n" + body.replace("source.alpha()", "source.other()") + "\n}\n",
        )
        def forced_hashes(function_ids, function_identifier):
            return iter(((0, 0, 123), (1, 0, 123)))
        with patch.object(ratchet, "_qualified_windows", side_effect=forced_hashes):
            duplicate_counts, _ = ratchet._duplicate_inventory(functions)
        self.assertEqual(duplicate_counts, {})

    def test_disjoint_blocks_charge_interval_probes_to_work_budget(self):
        left_parts = []
        right_parts = []
        for block_index in range(16):
            calls = [f"call{block_index}_{suffix}();" for suffix in "abcdef"]
            left_parts.extend(calls + [f"left_gap_{block_index};"])
            right_parts.extend(calls + [f"right_gap_{block_index};"])
        source = (
            "fn left() {\n" + "\n".join(left_parts) + "\n}\n"
            "fn right() {\n" + "\n".join(right_parts) + "\n}\n"
        )
        original_reserve = ratchet.WorkBudget.reserve

        def omit_interval_probe_charge(budget, amount):
            return original_reserve(budget, 1 if amount > 1 else amount)

        # This simulates the specific removed-interval-charge mutant; it stays below the bound.
        with patch.object(ratchet, "MAX_DUPLICATE_COMPARISONS", 200):
            with patch.object(ratchet.WorkBudget, "reserve", omit_interval_probe_charge):
                ratchet.analyze_sources([("disjoint.rs", source)])
            with self.assertRaisesRegex(ratchet.RatchetError, "comparison work limit"):
                ratchet.analyze_sources([("disjoint.rs", source)])

    def test_duplicate_group_storage_limit_refuses_instead_of_dropping_groups(self):
        body = repeated_block("bounded")
        source = (
            "fn first(source: Source) {\n" + body + "\n}\n"
            "fn second(source: Source) {\n" + body + "\n}\n"
        )
        with patch.object(ratchet, "MAX_DUPLICATE_BLOCK_GROUPS", 0):
            with self.assertRaisesRegex(ratchet.RatchetError, "block group limit"):
                ratchet.analyze_sources([("bounded.rs", source)])

    def test_all_three_ratchets_reject_metric_rises(self):
        base = ratchet.analyze_sources([("base.rs", "fn small() {}\n")])
        baseline = self.baseline_for(base)
        long_source = "fn long() {\n" + "\n".join("    // line" for _ in range(150)) + "\n}\n"
        long_now = ratchet.analyze_sources([("long.rs", long_source)])
        self.assertIn("long_function_count", " ".join(ratchet.compare(long_now, baseline)["errors"]))
        wide_now = ratchet.analyze_sources([("wide.rs", "fn wide(a,b,c,d,e,f,g,h) {}\n")])
        self.assertIn("wide_argument_function_count", " ".join(ratchet.compare(wide_now, baseline)["errors"]))
        duplicate = repeated_block("copy")
        dup_now = ratchet.analyze_sources([
            ("one.rs", "fn a(source: Source) {\n" + duplicate + "\n}\n"
             "fn b(source: Source) {\n" + duplicate + "\n}\n")
        ])
        self.assertIn("duplicate_block_count", " ".join(ratchet.compare(dup_now, baseline)["errors"]))

    def test_workspace_guard_rejects_missing_slash_and_unrelated_roots(self):
        with self.assertRaises(ratchet.RatchetError):
            ratchet._validate_workspace(Path("/path/that/does/not/exist"))
        with self.assertRaises(ratchet.RatchetError):
            ratchet._validate_workspace(Path("/"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            (root / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
            (root / "docs").mkdir()
            (root / "docs/RULESET.md").write_text("# unrelated project\n", encoding="utf-8")
            (root / "crates/oa-track").mkdir(parents=True)
            (root / "crates/oa-track/Cargo.toml").write_text("[package]\nname='other'\n", encoding="utf-8")
            (root / "tools").mkdir()
            (root / "tools/quality_size_ratchet.py").write_text("# not OpenATCA\n", encoding="utf-8")
            (root / "tools/oa-trace").mkdir()
            (root / "tools/oa-trace/Cargo.toml").write_text("[package]\nname='other-trace'\n", encoding="utf-8")
            (root / "tools/rust_proofs.json").write_text("{}\n", encoding="utf-8")
            subprocess.run(["git", "add", "Cargo.toml", "docs/RULESET.md", "crates/oa-track/Cargo.toml",
                            "tools/quality_size_ratchet.py", "tools/oa-trace/Cargo.toml",
                            "tools/rust_proofs.json"],
                           cwd=root, check=True)
            with self.assertRaises(ratchet.RatchetError):
                ratchet._validate_workspace(root)

    def test_actual_openatca_git_root_passes_workspace_binding(self):
        root = Path(__file__).resolve().parents[1]
        self.assertEqual(ratchet._validate_workspace(root), root.resolve())

    def test_malformed_baseline_file_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "baseline.json"
            path.write_text("{bad json", encoding="utf-8")
            with self.assertRaises(ratchet.RatchetError):
                ratchet._read_baseline(path)

    def test_baseline_writer_refuses_final_symlink_without_touching_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "outside.json"
            outside.write_bytes(b"original")
            link = root / "baseline.json"
            link.symlink_to(outside)
            with self.assertRaises(ratchet.RatchetError):
                ratchet._write_baseline(link, b"replacement")
            self.assertEqual(outside.read_bytes(), b"original")

    def test_baseline_writer_atomically_replaces_regular_output(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "baseline.json"
            target.write_bytes(b"original")
            ratchet._write_baseline(target, b"replacement")
            self.assertEqual(target.read_bytes(), b"replacement")
            self.assertEqual(list(Path(directory).iterdir()), [target])

    def test_baseline_binding_and_revision_fail_closed(self):
        summary = ratchet.analyze_sources([("sample.rs", "fn small() {}\n")])
        baseline = self.baseline_for(summary)
        self.assertEqual(ratchet.compare(summary, baseline)["status"], "PASS")
        for mutation in ({"workspace_binding": "other"}, {"seed_revision": ""}, {"schema": 2}):
            invalid = dict(baseline)
            invalid.update(mutation)
            with self.assertRaises(ratchet.RatchetError):
                ratchet.compare(summary, invalid)

    def test_comments_do_not_distinguish_duplicate_code_blocks(self):
        body = repeated_block("left")
        same_tokens_different_comments = body.replace("source.alpha()", "source.alpha() /* note one */")
        source = (
            "fn first(source: Source) {\n" + body + "\n}\n"
            "fn second(source: Source) {\n" + same_tokens_different_comments + "\n}\n"
        )
        summary = ratchet.analyze_sources([("sample.rs", source)])
        self.assertGreater(summary["duplicate_block_count"], 0)

    def test_added_duplicate_is_reported_even_when_total_does_not_rise(self):
        first = repeated_block("alpha")
        second = repeated_block("bravo")
        baseline_source = (
            "fn a(source: Source) {\n" + first + "\n}\n"
            "fn b(source: Source) {\n" + first + "\n}\n"
            "fn c(source: Source) {\n" + second + "\n}\n"
        )
        current_source = (
            "fn a(source: Source) {\n" + first + "\n}\n"
            "fn c(source: Source) {\n" + second + "\n}\n"
            "fn d(source: Source) {\n" + second + "\n}\n"
        )
        base = ratchet.analyze_sources([("sample.rs", baseline_source)])
        current = ratchet.analyze_sources([("sample.rs", current_source)])
        baseline = self.baseline_for(base)
        report = ratchet.compare(current, baseline)
        self.assertEqual(current["duplicate_block_count"], base["duplicate_block_count"])
        self.assertEqual(report["status"], "PASS")
        self.assertTrue(report["new_or_increased_duplicates"])
        baseline_ids = set(base["duplicate_frequencies"])
        self.assertTrue(any(row["fingerprint"] not in baseline_ids
                            for row in report["new_or_increased_duplicates"]))

    def test_ratchets_accept_equal_or_reduced_counts_and_reject_rises(self):
        clean = ratchet.analyze_sources([("sample.rs", "fn small() {}\n")])
        baseline = self.baseline_for(clean)
        self.assertEqual(ratchet.compare(clean, baseline)["status"], "PASS")
        wide = "fn wide(a,b,c,d,e,f,g,h) {}\n"
        elevated = ratchet.analyze_sources([("sample.rs", wide)])
        report = ratchet.compare(elevated, baseline)
        self.assertEqual(report["status"], "FAIL")
        self.assertTrue(any("wide_argument_function_count" in error for error in report["errors"]))


if __name__ == "__main__":
    unittest.main()
