"""Synthetic end-to-end tests for source-bound, clause-level row claims."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_done_claim as checker  # noqa: E402


SOURCE = (
    "# Synthetic acceptance source\n\n"
    "| ID | Required | Source | Type | Status | Gap / next step |\n"
    "|---|---|---|---|---|---|\n"
    "| TEST-A | Synthetic alpha; beta condition. | TEST | E | Partial | Synthetic gap |\n"
    "| TEST-B | Separate required statement. | TEST | E | Unknown | Synthetic |\n"
)
COMMIT = "1" * 40


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class ClaimCheckerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source_bytes = SOURCE.encode()
        self.rows = checker.parse_faa_acceptance(SOURCE)
        self.source_digest = digest(self.source_bytes)
        self.test_path = self.root / "tests.rs"
        self.test_path.write_text(
            "#[test]\nfn test_alpha() {}\n#[test]\nfn test_beta() {}\n",
            encoding="utf-8",
        )
        self.trace = self.root / "trace.md"
        self.trace_bytes = (
            "# Trace\n\n| Requirement | Status | Tests |\n|---|---|---|\n"
            "| OA-GEN-001 | test linked | `test_alpha` (tests.rs:2)<br>`test_beta` (tests.rs:4) |\n"
        ).encode()
        self.trace.write_bytes(self.trace_bytes)
        manifest_rows = []
        for row_id, text in self.rows.items():
            if row_id == "TEST-A":
                split = text.index(";")
                spans = [{"start": 0, "end": split}, {"start": split, "end": len(text)}]
            else:
                spans = [{"start": 0, "end": len(text)}]
            manifest_rows.append({"id": row_id, "text_sha256": digest(text.encode()), "clauses": spans})
        self.manifest = {
            "schema": checker.MANIFEST_SCHEMA,
            "source_sha256": self.source_digest,
            "requirements": manifest_rows,
        }
        self.manifest_bytes = json.dumps(self.manifest, sort_keys=True, separators=(",", ":")).encode()
        self.manifest_digest = digest(self.manifest_bytes)
        self.spans = self.manifest["requirements"][0]["clauses"]
        clause_digests = [
            digest(self.rows["TEST-A"][span["start"] : span["end"]].encode()) for span in self.spans
        ]
        self.test_entries = [
            {
                "requirement_id": "OA-GEN-001",
                "path": "tests.rs",
                "symbol": name,
                "line": line,
                "source_sha256": digest(self.test_path.read_bytes()),
                "clause_sha256": clause_digests[index],
            }
            for index, (name, line) in enumerate((("test_alpha", 2), ("test_beta", 4)))
        ]
        self.inventory = {
            "schema": checker.INVENTORY_SCHEMA,
            "producer": "oa-trace-clause-review",
            "source_commit": COMMIT,
            "manifest_sha256": self.manifest_digest,
            "scope_sha256": checker._scope_digest(self.source_digest, self.manifest_digest, "TEST-A", self.spans),
            "trace_report_path": "trace.md",
            "trace_report_sha256": digest(self.trace_bytes),
            "validator_sha256": "0" * 64,
            "tests": self.test_entries,
        }
        self.inventory_bytes = json.dumps(self.inventory, sort_keys=True, separators=(",", ":")).encode()
        self.claim = {
            "schema": checker.CLAIM_SCHEMA,
            "source_sha256": self.source_digest,
            "manifest_sha256": self.manifest_digest,
            "test_inventory_sha256": digest(self.inventory_bytes),
            "row_id": "TEST-A",
            "clauses": [
                {
                    **span,
                    "status": "covered",
                    "evidence": [
                        {
                            "kind": "test",
                            "requirement_id": "OA-GEN-001",
                            "path": "tests.rs",
                            "line": 2 if index == 0 else 4,
                            "symbol": "test_alpha" if index == 0 else "test_beta",
                            "clause_sha256": clause_digests[index],
                        }
                    ],
                }
                for index, span in enumerate(self.spans)
            ],
        }

    def tearDown(self) -> None:
        self.temp.cleanup()

    def assert_cli_refused(self, command, env, expected_message=None):
        result = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        if expected_message is not None:
            self.assertIn(expected_message, result.stderr)
        return result

    def assert_claim_refused(self, message, **values):
        with self.assertRaisesRegex(checker.ClaimError, message):
            self.validate(**values)

    def validate(
        self,
        claim: dict | None = None,
        manifest: dict | None = None,
        inventory: dict | None = None,
        rebind: bool = False,
    ) -> None:
        manifest_value = manifest or self.manifest
        inventory_value = inventory or self.inventory
        manifest_bytes = json.dumps(manifest_value, sort_keys=True, separators=(",", ":")).encode()
        inventory_bytes = json.dumps(inventory_value, sort_keys=True, separators=(",", ":")).encode()
        claim_value = json.loads(json.dumps(claim or self.claim))
        if rebind:
            claim_value["source_sha256"] = self.source_digest
            claim_value["manifest_sha256"] = digest(manifest_bytes)
            claim_value["test_inventory_sha256"] = digest(inventory_bytes)
        checker.validate_row_claim(checker.RowClaimInputs(
            root=self.root,
            source_bytes=self.source_bytes,
            rows_by_id=self.rows,
            manifest=manifest_value,
            manifest_bytes=manifest_bytes,
            claim=claim_value,
            inventory=inventory_value,
            inventory_bytes=inventory_bytes,
            row_id="TEST-A",
            source_commit=COMMIT,
        ))

    # Mutation: omit or duplicate authoritative row clauses in the required registry.
    def test_manifest_must_cover_all_source_rows_and_all_clause_text(self) -> None:
        missing_row = {**self.manifest, "requirements": self.manifest["requirements"][:1]}
        with self.assertRaisesRegex(checker.ClaimError, "omits or invents"):
            self.validate(manifest=missing_row)
        gapped = json.loads(json.dumps(self.manifest))
        gapped["requirements"][0]["clauses"][0]["end"] -= 1
        with self.assertRaisesRegex(checker.ClaimError, "gap, overlap"):
            self.validate(manifest=gapped)
        duplicate = json.loads(json.dumps(self.manifest))
        duplicate["requirements"].append(duplicate["requirements"][0])
        with self.assertRaisesRegex(checker.ClaimError, "duplicate"):
            self.validate(manifest=duplicate)

    def test_structural_validation_accepts_complete_synthetic_bindings(self) -> None:
        self.validate()

    # Mutation: weaken manifest text or claim binding while retaining a complete-looking claim.
    def test_source_and_manifest_digests_are_required_bindings(self) -> None:
        weak = json.loads(json.dumps(self.manifest))
        weak["requirements"][0]["text_sha256"] = "0" * 64
        with self.assertRaisesRegex(checker.ClaimError, "text binding mismatch"):
            self.validate(manifest=weak)
        claim = json.loads(json.dumps(self.claim))
        claim["manifest_sha256"] = "0" * 64
        with self.assertRaisesRegex(checker.ClaimError, "not bound"):
            self.validate(claim=claim)

    # Mutation: allow a missing, repeated, partial, or external clause to pass a Done check.
    def test_claim_must_match_every_canonical_clause_and_cover_all(self) -> None:
        claim = json.loads(json.dumps(self.claim))
        claim["clauses"].pop()
        self.assert_claim_refused("omits or duplicates", claim=claim)
        claim = json.loads(json.dumps(self.claim))
        claim["clauses"][1]["status"] = "partial"
        self.assert_claim_refused("refused", claim=claim)
        claim["clauses"][1]["status"] = "external"
        self.assert_claim_refused("refused", claim=claim)
        claim["clauses"][1]["status"] = "unmapped"
        self.assert_claim_refused("refused", claim=claim)

    # Mutation: cite one valid test for an entire requirement and omit evidence for another clause.
    def test_each_clause_needs_its_own_trace_bound_test(self) -> None:
        claim = json.loads(json.dumps(self.claim))
        claim["clauses"][1]["evidence"] = []
        self.assert_claim_refused("has no evidence", claim=claim)
        claim["clauses"][1]["evidence"] = [claim["clauses"][0]["evidence"][0]]
        self.assert_claim_refused("binds different wording", claim=claim)

    # Mutation: accept a fabricated test symbol, mismatched trace, or stale test-source hash.
    def test_test_evidence_must_exist_in_bound_inventory_and_trace(self) -> None:
        cases = (
            ("symbol", "made_up_helper", "absent from the bound oa-trace"),
            ("source_sha256", "0" * 64, "source digest mismatch"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                inventory = json.loads(json.dumps(self.inventory))
                inventory["tests"][0][field] = value
                self.assert_claim_refused(message, inventory=inventory, rebind=True)

    # Mutation: let an inventory from another commit or row scope support this claim.
    def test_inventory_is_bound_to_commit_manifest_and_exact_row_scope(self) -> None:
        cases = (
            ("source_commit", "2" * 40, "current source commit"),
            ("scope_sha256", "0" * 64, "row and clause scope"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                inventory = json.loads(json.dumps(self.inventory))
                inventory[field] = value
                self.assert_claim_refused(message, inventory=inventory, rebind=True)

    # Mutation: allow a removed row-claim file or absent canonical inputs to silently skip checking.
    def test_cli_requires_manifest_claim_inventory_and_requested_row(self) -> None:
        script = Path(checker.__file__)
        result = subprocess.run([sys.executable, str(script), "--row", "TEST-A"], capture_output=True, check=False)
        self.assertNotEqual(result.returncode, 0)

    # Mutation: expose a checker that passes unit helpers but is disconnected from normal row CLI use.
    def test_faa_cli_checks_claim_with_trusted_trace_binary_or_refuses_missing_tool(self) -> None:
        repo = Path(__file__).resolve().parents[1]
        private = self.root / "private-input"
        private.mkdir()
        (private / "FAA_ACCEPTANCE_LIST.md").write_bytes(self.source_bytes)
        manifest_path = private / "manifest.json"
        manifest_path.write_bytes(self.manifest_bytes)
        source_commit = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "--verify", "HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        source_path = "crates/oa-geo/tests/general.rs"
        source_file = (repo / source_path).read_bytes()
        symbol = "every_source_file_is_unsafe_free_and_lib_forbids_unsafe"
        source_line = 13
        fixture_trace = (
            "| Requirement | Status | Tests |\n|---|---|---|\n"
            f"| OA-GEN-001 | test linked | `{symbol}` ({source_path}:{source_line}) |\n"
        ).encode()
        trace_path = repo / "target/e05-claim-checker-fixture/trace.md"
        trace_path.parent.mkdir(parents=True, exist_ok=True)
        trace_path.write_bytes(fixture_trace)
        clause_digests = [
            digest(self.rows["TEST-A"][span["start"] : span["end"]].encode()) for span in self.spans
        ]
        test_entries = [
            {
                "requirement_id": "OA-GEN-001", "path": source_path, "symbol": symbol,
                "line": source_line, "source_sha256": digest(source_file), "clause_sha256": clause_digest,
            }
            for clause_digest in clause_digests
        ]
        validator_value = os.environ.get("OPENATCA_OA_TRACE_BIN", "/missing/oa-trace")
        validator_path = Path(validator_value)
        validator_digest = digest(validator_path.read_bytes()) if validator_path.is_file() else "0" * 64
        inventory = {
            **self.inventory,
            "source_commit": source_commit,
            "tests": test_entries,
            "trace_report_path": "target/e05-claim-checker-fixture/trace.md",
            "trace_report_sha256": digest(fixture_trace),
            "validator_sha256": validator_digest,
        }
        inventory_bytes = json.dumps(inventory, sort_keys=True, separators=(",", ":")).encode()
        claim = {
            **self.claim,
            "test_inventory_sha256": digest(inventory_bytes),
            "clauses": [
                {
                    **span,
                    "status": "covered",
                    "evidence": [{
                        "kind": "test", "requirement_id": "OA-GEN-001", "path": source_path,
                        "line": source_line, "symbol": symbol, "clause_sha256": clause_digests[index],
                    }],
                }
                for index, span in enumerate(self.spans)
            ],
        }
        claim_path = private / "claim.json"
        claim_path.write_text(json.dumps(claim), encoding="utf-8")
        inventory_path = private / "inventory.json"
        inventory_path.write_bytes(inventory_bytes)
        command = [
            sys.executable, str(Path(checker.__file__)), "--source-kind", "faa", "--repo", str(repo),
            "--manifest", str(manifest_path), "--claim", str(claim_path),
            "--test-inventory", str(inventory_path), "--row", "TEST-A", "--source-commit", source_commit,
            "--expected-source-sha256", self.source_digest,
            "--oa-trace-bin", str(validator_path),
        ]
        env = os.environ.copy()
        env["OPENATCA_PRIVATE"] = str(private)
        data_root = self.root / "synthetic-data-root"
        data_root.mkdir()
        (data_root / ".openatca-data-root").write_bytes(b"openatca-data-root-v1\n")
        env["OPENATCA_DATA"] = str(data_root)
        result = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
        if validator_path.is_file() and os.access(validator_path, os.X_OK):
            self.assertEqual(result.returncode, 0, result.stderr)
            claim["clauses"][1]["status"] = "external"
            claim_path.write_text(json.dumps(claim), encoding="utf-8")
            refused = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("refused", refused.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("validator is not executable", result.stderr)

    # Mutation: let absent, wrong, or root-valued private path configuration fall through.
    def test_faa_cli_fails_closed_on_private_root_configuration(self) -> None:
        command = [
            sys.executable, str(Path(checker.__file__)), "--source-kind", "faa",
            "--repo", str(self.root), "--manifest", "manifest.json", "--claim", "claim.json",
            "--test-inventory", "inventory.json", "--row", "TEST-A", "--source-commit", COMMIT,
            "--oa-trace-bin", "/missing/oa-trace",
        ]
        wrong = self.root / "wrong-private-root"
        wrong.mkdir()
        (wrong / "FAA_ACCEPTANCE_LIST.md").write_text("no acceptance table\n", encoding="utf-8")
        envs = []
        missing = os.environ.copy()
        missing.pop("OPENATCA_PRIVATE", None)
        envs.append((command, missing, "OPENATCA_PRIVATE is required"))
        envs.append((command, {**os.environ, "OPENATCA_PRIVATE": "/"}, "allowed private data root"))
        envs.append((command, {**os.environ, "OPENATCA_PRIVATE": str(wrong)}, "independently supplied digest"))
        command.extend(["--expected-source-sha256", self.source_digest])
        envs.append((command, {**os.environ, "OPENATCA_PRIVATE": str(wrong)}, "source differs"))
        envs.append((command[:-1] + ["0" * 64], {**os.environ, "OPENATCA_PRIVATE": str(wrong)}, None))
        for args, env, message in envs:
            with self.subTest(message=message):
                self.assert_cli_refused(args, env, message)

    def test_bounded_reader_stops_before_reading_past_cap(self) -> None:
        evidence = self.root / "large.txt"
        evidence.write_bytes(b"12345")
        with self.assertRaisesRegex(checker.ClaimError, "byte cap"):
            checker._bounded_read(evidence, 4, "synthetic input")

    # Mutation: permit evidence paths to escape the repository through symlinks or oversized reads.
    def test_evidence_read_is_bounded_and_rejects_symlink_escape(self) -> None:
        missing = {
            "kind": "source", "path": "no-evidence.py", "start_line": 1, "end_line": 1,
            "symbol": "missing_symbol", "sha256": "0" * 64,
        }
        with self.assertRaisesRegex(checker.ClaimError, "evidence file is missing"):
            checker._evidence(self.root, missing, 0)
        outside = self.root.parent / (self.root.name + "-outside")
        outside.write_text("external evidence", encoding="utf-8")
        link = self.root / "escape.txt"
        try:
            link.symlink_to(outside)
            evidence = {
                "kind": "source", "path": "escape.txt", "start_line": 1, "end_line": 1,
                "symbol": "external", "sha256": digest(outside.read_bytes()),
            }
            with self.assertRaisesRegex(checker.ClaimError, "escapes repository"):
                checker._evidence(self.root, evidence, 0)
        finally:
            outside.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
