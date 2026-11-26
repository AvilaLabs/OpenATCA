import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import generate_safety_tables as gen


def fixture():
    return {
        "schema": "openatca.hazard-log",
        "assessment_status": "DRAFT_NOT_ASSESSED",
        "coverage_status": "DECLARED_FUNCTION_SCOPE_ONLY",
        "safety_requirement_ids": ["OA-TEST-001"],
        "safety_requirement_assessments": [{
            "id": "OA-TEST-001", "function": "func, with | delimiter",
            "hazard_id": "HZ-TEST-001", "rationale": "test",
        }],
        "hazards": [{
            "id": "HZ-TEST-001", "function": "func, with | delimiter",
            "cause": "cause,\nwith newline", "effect": "effect | value",
            "severity_class": "Major", "severity_status": "engineering_candidate",
            "mitigation_requirement_ids": ["OA-TEST-001"],
            "verification_refs": [{"requirement_id": "OA-TEST-001", "file": "tests/a,b.rs",
                                   "test_name": "test | name"}],
            "proof_refs": [],
        }],
    }


class GeneratorTests(unittest.TestCase):
    def test_missing_mitigation_fails(self):
        data = fixture()
        data["hazards"][0]["mitigation_requirement_ids"] = []
        with self.assertRaisesRegex(ValueError, "missing mitigation"):
            gen.records(data)

    def test_reference_hazard_mismatch_fails(self):
        data = fixture()
        data["hazards"][0]["id"] = "HZ-OTHER"
        with self.assertRaisesRegex(ValueError, "does not match its assessment"):
            gen.records(data)

    def test_csv_escaping_and_markdown_cells(self):
        rows = gen.records(fixture())[0][1]
        encoded = gen.render_csv(rows)
        self.assertIn('"cause,\nwith newline"', encoded)
        self.assertIn('"func, with | delimiter"', encoded)
        self.assertIn("effect \\| value", gen.render_markdown(rows, fixture()))
        self.assertIn("| Severity class | Severity status |", gen.render_markdown(rows, fixture()))
        self.assertIn("Assessment status: DRAFT_NOT_ASSESSED | Coverage status: DECLARED_FUNCTION_SCOPE_ONLY",
                      gen.render_markdown(rows, fixture()))

    def test_sorted_deterministic_output(self):
        data = fixture()
        second = copy.deepcopy(data["hazards"][0])
        second.update(id="HZ-TEST-002", function="z", mitigation_requirement_ids=["OA-TEST-001"],
                      verification_refs=[])
        data["hazards"].append(second)
        outputs_a = [(name, gen.render_csv(rows), gen.render_markdown(rows, data))
                     for name, rows in gen.records(data)]
        data["hazards"].reverse()
        outputs_b = [(name, gen.render_csv(rows), gen.render_markdown(rows, data))
                     for name, rows in gen.records(data)]
        self.assertEqual(outputs_a, outputs_b)

    def test_bounded_read_accepts_exact_byte_limit_and_rejects_one_over_before_parse(self):
        encoded = json.dumps(fixture()).encode("utf-8")
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "hazards.json"
            path.write_bytes(encoded)
            with mock.patch.object(gen, "MAX_LOG_BYTES", len(encoded)):
                self.assertEqual(gen._bounded_read(path)["schema"], "openatca.hazard-log")
            with mock.patch.object(gen, "MAX_LOG_BYTES", len(encoded) - 1), \
                 mock.patch.object(gen.json, "loads", side_effect=AssertionError("parsed oversized input")):
                with self.assertRaisesRegex(ValueError, "input exceeds"):
                    gen._bounded_read(path)

    def test_json_depth_and_node_limits_accept_boundary_and_reject_over(self):
        with mock.patch.object(gen, "MAX_JSON_DEPTH", 4), mock.patch.object(gen, "MAX_JSON_NODES", 20):
            gen._preflight_json(b"[[[[0]]]]", "fixture")
            with self.assertRaisesRegex(ValueError, "nesting exceeds"):
                gen._preflight_json(b"[[[[[0]]]]]", "fixture")
        with mock.patch.object(gen, "MAX_JSON_DEPTH", 16), mock.patch.object(gen, "MAX_JSON_NODES", 3):
            gen._preflight_json(b"[0,0]", "fixture")
            with self.assertRaisesRegex(ValueError, "nodes exceed"):
                gen._preflight_json(b"[0,0,0]", "fixture")

    def test_text_cell_and_total_text_limits_accept_boundary_and_reject_over(self):
        with mock.patch.object(gen, "MAX_TEXT_CELL_BYTES", 5), mock.patch.object(gen, "MAX_TOTAL_TEXT_BYTES", 8):
            gen._preflight_json(b'"12345"', "fixture")
            with self.assertRaisesRegex(ValueError, "string exceeds"):
                gen._preflight_json(b'"123456"', "fixture")
            gen._preflight_json(b'["1234","5678"]', "fixture")
            with self.assertRaisesRegex(ValueError, "text exceeds"):
                gen._preflight_json(b'["1234","56789"]', "fixture")

    def test_hazard_and_requirement_counts_accept_boundary_and_reject_over(self):
        data = fixture()
        with mock.patch.object(gen, "MAX_HAZARDS", 1), mock.patch.object(gen, "MAX_REQUIREMENTS", 1):
            gen.records(data)
            data["hazards"].append(copy.deepcopy(data["hazards"][0]))
            with self.assertRaisesRegex(ValueError, "hazards exceed 1-item limit"):
                gen.records(data)
            data = fixture()
            data["safety_requirement_ids"].append("OA-TEST-002")
            with self.assertRaisesRegex(ValueError, "requirements exceed 1-item limit"):
                gen.records(data)

    def test_reference_count_limit_and_near_limit(self):
        data = fixture()
        data["hazards"][0]["proof_refs"] = [
            {"requirement_id": "OA-TEST-001", "file": "proof.rs", "proof_name": "proof"}
        ]
        with mock.patch.object(gen, "MAX_REFERENCES", 2):
            self.assertEqual(len(gen.records(data)[2][1]), 1)
            data["hazards"][0]["proof_refs"].append(copy.deepcopy(data["hazards"][0]["proof_refs"][0]))
            with self.assertRaisesRegex(ValueError, "references exceed 2-item limit"):
                gen.records(data)

    def test_rejected_input_does_not_create_or_partially_write_output(self):
        data = fixture()
        data["hazards"][0]["proof_refs"] = [
            {"requirement_id": "OA-TEST-001", "file": "proof.rs", "proof_name": "proof"}
        ]
        with tempfile.TemporaryDirectory() as temp_dir:
            input_path = Path(temp_dir) / "hazards.json"
            output_path = Path(temp_dir) / "generated"
            input_path.write_text(json.dumps(data), encoding="utf-8")
            with mock.patch.object(gen, "MAX_REFERENCES", 1):
                with self.assertRaisesRegex(ValueError, "references exceed"):
                    gen.generate(log_path=input_path, out=output_path)
            self.assertFalse(output_path.exists())

    def test_output_budget_is_checked_before_render_or_output_creation(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            input_path = Path(temp_dir) / "hazards.json"
            output_path = Path(temp_dir) / "generated"
            input_path.write_text(json.dumps(fixture()), encoding="utf-8")
            with mock.patch.object(gen, "MAX_GENERATED_BYTES", 1), \
                 mock.patch.object(gen, "render_markdown", side_effect=AssertionError("rendered before budget")), \
                 mock.patch.object(gen, "render_csv", side_effect=AssertionError("rendered before budget")):
                with self.assertRaisesRegex(ValueError, "estimated generated tables exceed"):
                    gen.generate(log_path=input_path, out=output_path)
            self.assertFalse(output_path.exists())

    def test_estimated_output_budget_covers_both_renderers(self):
        data = fixture()
        for _, rows in gen.records(data):
            rendered_bytes = sum(len(content.encode("utf-8")) for content in (
                gen.render_markdown(rows, data), gen.render_csv(rows)))
            self.assertGreaterEqual(gen._estimated_artifact_bytes(rows), rendered_bytes)


if __name__ == "__main__":
    unittest.main()
