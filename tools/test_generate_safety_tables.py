import copy
import unittest

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


if __name__ == "__main__":
    unittest.main()
