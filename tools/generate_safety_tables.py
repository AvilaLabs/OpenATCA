#!/usr/bin/env python3
"""Generate deterministic, factual safety register tables from the declared safety log."""

import argparse
import csv
import io
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LOG = ROOT / "docs/safety/hazard-log.json"
OUT = ROOT / "docs/safety/generated"


def load_log(path=LOG):
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != "openatca.hazard-log":
        raise ValueError("unexpected hazard log schema")
    if data.get("assessment_status") != "DRAFT_NOT_ASSESSED":
        raise ValueError("unexpected assessment status")
    if data.get("coverage_status") != "DECLARED_FUNCTION_SCOPE_ONLY":
        raise ValueError("unexpected coverage status")
    return data


def records(data):
    hazards = sorted(data["hazards"], key=lambda row: row["id"])
    assessments = {row["id"]: row for row in data["safety_requirement_assessments"]}
    hazards_by_id = {row["id"]: row for row in hazards}
    if len(assessments) != len(data["safety_requirement_ids"]):
        raise ValueError("safety requirement assessment/register count mismatch")
    if set(assessments) != set(data["safety_requirement_ids"]):
        raise ValueError("safety requirement assessment/register IDs mismatch")
    for hazard in hazards:
        for kind in ("verification_refs", "proof_refs"):
            for ref in hazard.get(kind, []):
                req_id = ref["requirement_id"]
                if req_id not in assessments or assessments[req_id]["hazard_id"] != hazard["id"]:
                    raise ValueError(f"{req_id}: {kind} hazard reference does not match its assessment")
    if len(hazards_by_id) != len(hazards):
        raise ValueError("duplicate hazard ID")

    pha = []
    osa = []
    ssar = []
    for hazard in hazards:
        mitigations = sorted(set(hazard.get("mitigation_requirement_ids", [])))
        if not mitigations:
            raise ValueError(f"{hazard['id']}: missing mitigation requirement link")
        pha.append({
            "Hazard ID": hazard["id"], "Function": hazard["function"],
            "Cause": hazard["cause"], "Effect": hazard["effect"],
            "Severity class": hazard["severity_class"],
            "Severity status": hazard["severity_status"],
            "Likelihood": "Not yet assessed",
            "Initial mitigations": "; ".join(mitigations),
            "Assessment status": data["assessment_status"],
        })
        osa.append({"Function": hazard["function"], "Hazard ID": hazard["id"],
                    "Hazard": hazard["effect"]})

    for req_id in sorted(data["safety_requirement_ids"]):
        assessment = assessments[req_id]
        hazard = hazards_by_id.get(assessment["hazard_id"])
        if hazard is None:
            raise ValueError(f"{req_id}: assessment references unknown hazard {assessment['hazard_id']}")
        verifications = [r for r in hazard.get("verification_refs", []) if r["requirement_id"] == req_id]
        proofs = [r for r in hazard.get("proof_refs", []) if r["requirement_id"] == req_id]
        if not verifications and not proofs:
            raise ValueError(f"{req_id}: missing oa-trace verification/proof reference")
        links = []
        for ref in sorted(verifications, key=lambda r: (r["file"], r["test_name"])):
            links.append(f"test: {ref['file']}#{ref['test_name']}")
        for ref in sorted(proofs, key=lambda r: (r["file"], r["proof_name"])):
            links.append(f"proof: {ref['file']}#{ref['proof_name']}")
        methods = ["Test"] if verifications else ["Proof"]
        if verifications and proofs:
            methods = ["Test", "Proof"]
        ssar.append({"Requirement ID": req_id, "Function": assessment["function"],
                     "Hazard ID": hazard["id"], "Verification method": "; ".join(methods),
                     "Test/proof links": "; ".join(links),
                     "Assessment status": data["assessment_status"]})
    return [("pha", pha), ("osa", osa), ("ssar", ssar)]


def render_csv(rows):
    stream = io.StringIO(newline="")
    writer = csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator="\n")
    writer.writeheader()
    writer.writerows(rows)
    return stream.getvalue()


def render_markdown(rows, data):
    columns = list(rows[0])
    def cell(value):
        return str(value).replace("|", "\\|").replace("\r", " ").replace("\n", "<br>")
    lines = [
        "<!-- Generated from docs/safety/hazard-log.json and its oa-trace references. -->",
        "",
        f"**Assessment status: {data['assessment_status']} | Coverage status: {data['coverage_status']} | Not an accredited SSA.**",
        "",
        "| " + " | ".join(columns) + " |",
        "| " + " | ".join("---" for _ in columns) + " |",
    ]
    lines.extend("| " + " | ".join(cell(row[key]) for key in columns) + " |" for row in rows)
    return "\n".join(lines) + "\n"


def generate(check=False, log_path=LOG, out=OUT):
    data = load_log(log_path)
    artifacts = records(data)
    out.mkdir(parents=True, exist_ok=True)
    stale = []
    for name, rows in artifacts:
        for ext, content in (("md", render_markdown(rows, data)), ("csv", render_csv(rows))):
            path = out / f"{name}.{ext}"
            if check:
                if not path.is_file() or path.open(encoding="utf-8", newline="").read() != content:
                    stale.append(str(path.relative_to(ROOT)))
            else:
                with path.open("w", encoding="utf-8", newline="") as stream:
                    stream.write(content)
    if stale:
        raise ValueError("generated safety tables are stale or missing: " + ", ".join(stale))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if generated files are stale")
    args = parser.parse_args()
    try:
        generate(check=args.check)
    except (ValueError, KeyError, OSError, json.JSONDecodeError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
