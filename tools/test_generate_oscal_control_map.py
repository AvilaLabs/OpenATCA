import unittest
from pathlib import Path

from generate_oscal_control_map import (
    SOURCE_BYTES,
    SOURCE_SHA256,
    source_inventory,
    render_csv,
    render_markdown,
    summaries,
    validate_map,
    generate,
)


def source_fixture():
    return {
        "catalog": {
            "metadata": {
                "version": "5.2.0",
                "oscal-version": "1.2.2",
                "title": "NIST 800-53 HIGH IMPACT BASELINE test source",
            },
            "groups": [
                {
                    "id": "ac",
                    "controls": [
                        {
                            "id": "ac-1",
                            "title": "Policy and Procedures",
                            "params": [{"id": "ac-1_prm_1"}],
                            "controls": [
                                {"id": "ac-1.1", "title": "Enhancement", "params": []}
                            ],
                        }
                    ],
                }
            ],
        }
    }


def map_fixture(rows):
    return {
        "schema": "openatca.oscal-control-map",
        "version": 1,
        "source": {"sha256": SOURCE_SHA256, "bytes": SOURCE_BYTES},
        "controls": rows,
    }


def row(control_id, oscal_id, title, parameter_ids=None, status="planned", evidence=None):
    return {
        "control_id": control_id,
        "oscal_id": oscal_id,
        "family": "AC",
        "title": title,
        "parameter_ids": parameter_ids or [],
        "status": status,
        "evidence_refs": (evidence or []) if status != "implemented" else [],
        "implemented_refs": evidence if status == "implemented" else [],
        "missing_facts": ["assessment pending"],
    }


class ControlMapGeneratorTests(unittest.TestCase):
    # REQ: OA-E1-001
    def test_inventory_includes_nested_enhancements_and_parameter_ids(self):
        rows = source_inventory(source_fixture())
        self.assertEqual(
            rows,
            [
                {
                    "control_id": "AC-1",
                    "oscal_id": "ac-1",
                    "family": "AC",
                    "title": "Policy and Procedures",
                    "parameter_ids": ["ac-1_prm_1"],
                },
                {
                    "control_id": "AC-1(1)",
                    "oscal_id": "ac-1.1",
                    "family": "AC",
                    "title": "Enhancement",
                    "parameter_ids": [],
                },
            ],
        )

    # REQ: OA-E1-001
    def test_inventory_rejects_duplicate_source_control_ids(self):
        duplicate = source_fixture()
        duplicate["catalog"]["groups"][0]["controls"].append(
            {"id": "ac-1", "title": "Duplicate Policy", "params": []}
        )
        with self.assertRaisesRegex(ValueError, "duplicate OSCAL control id in source"):
            source_inventory(duplicate)

    # REQ: OA-E1-001
    def test_map_rejects_missing_and_duplicate_controls(self):
        source_rows = source_inventory(source_fixture())
        base = row("AC-1", "ac-1", "Policy and Procedures", ["ac-1_prm_1"])
        with self.assertRaisesRegex(ValueError, "inventory mismatch: missing="):
            validate_map(source_rows, map_fixture([base]))
        enhancement = row("AC-1(1)", "ac-1.1", "Enhancement")
        with self.assertRaisesRegex(ValueError, "missing or duplicate control-map id"):
            validate_map(source_rows, map_fixture([base, enhancement, enhancement.copy()]))

    # REQ: OA-E1-002
    def test_implemented_requires_exact_test_or_ci_reference(self):
        source_rows = source_inventory(source_fixture())
        implemented = row(
            "AC-1",
            "ac-1",
            "Policy and Procedures",
            ["ac-1_prm_1"],
            status="implemented",
            evidence=[
                {
                    "kind": "rust_test",
                    "path": "tests/security.rs",
                    "test_name": "implementation_is_checked",
                }
            ],
        )
        enhancement = row("AC-1(1)", "ac-1.1", "Enhancement")
        self.assertEqual(
            len(validate_map(source_rows, map_fixture([implemented, enhancement]))), 2
        )
        implemented["evidence_refs"] = []
        implemented["implemented_refs"] = []
        with self.assertRaisesRegex(ValueError, "implemented status requires an exact test or CI reference"):
            validate_map(source_rows, map_fixture([implemented, enhancement]))

    # REQ: OA-E1-003
    def test_counts_are_deterministic_by_family_and_status(self):
        rows = [
            row("AC-1", "ac-1", "Policy and Procedures", status="planned"),
            row("AC-1(1)", "ac-1.1", "Enhancement", status="inherited"),
        ]
        counts = summaries(rows)
        self.assertEqual(counts["AC"]["planned"], 1)
        self.assertEqual(counts["AC"]["inherited"], 1)
        self.assertEqual(sorted(counts), ["AC"])
        self.assertIn("| AC | 2 | 0 | 0 | 1 | 1 |", render_markdown(counts, rows))
        self.assertEqual(
            render_csv(counts),
            "family,total,implemented,partial,planned,inherited\nAC,2,0,0,1,1\nTOTAL,2,0,0,1,1\n",
        )

    # REQ: OA-E1-001
    def test_checked_in_inventory_matches_the_pinned_source_and_reports(self):
        root = Path(__file__).resolve().parents[1]
        generate(root, check=True)


if __name__ == "__main__":
    unittest.main()
