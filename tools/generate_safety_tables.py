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

# Current register profile: 1,695,053 bytes, depth 6, 32,489 JSON nodes,
# 1,417,703 bytes of text (largest cell 12,243 bytes), 20 coarse hazards,
# 1,094 requirements/assessments, 1,184 references and 1,094 mitigation links.
# Caps provide 20-24% growth in bytes/nodes/requirements, 20% more hazards,
# 35% more references and 34% more bytes per cell. Depth 16 allows ten further
# nesting levels. Parsing stays within 2 MiB input, 40k nodes, 1.75 MiB text.
# The work formula below is 13,648 units for today's profile and 17,152 at all
# collection caps; an 18k ceiling leaves a small explicit margin. Output is
# conservatively estimated before rendering and capped at 2 MiB.
MAX_LOG_BYTES = 2 * 1024 * 1024
MAX_JSON_DEPTH = 16
MAX_JSON_NODES = 40_000
MAX_TEXT_CELL_BYTES = 16 * 1024
MAX_TOTAL_TEXT_BYTES = 1_792 * 1024
MAX_HAZARDS = 24
MAX_REQUIREMENTS = 1_320
MAX_REFERENCES = 1_600
MAX_MITIGATIONS = 1_320
MAX_WORK_UNITS = 18_000
# Current tables total 490,336 bytes; today's conservative estimate is
# 1,525,329 bytes after worst-case CSV quoting, Markdown escaping and framing.
# The 2 MiB ceiling leaves about 36% headroom over that computed bound while
# limiting staged output memory to a small fixed amount.
MAX_GENERATED_BYTES = 2 * 1024 * 1024


def _preflight_json(raw, source):
    """Bound JSON nesting and lexical work before json.loads builds objects."""
    depth = 0
    nodes = 0
    text_bytes = 0
    index = 0
    while index < len(raw):
        byte = raw[index]
        if byte == ord('"'):
            nodes += 1
            if nodes > MAX_JSON_NODES:
                raise ValueError(f"{source}: JSON nodes exceed {MAX_JSON_NODES}-node limit")
            index += 1
            cell_bytes = 0
            while index < len(raw):
                byte = raw[index]
                if byte == ord('\\'):
                    # Count escaped bytes as written; this is conservative for
                    # unicode escapes and keeps the scan independent of parsing.
                    cell_bytes += 2
                    index += 2
                    continue
                if byte == ord('"'):
                    index += 1
                    break
                cell_bytes += 1
                index += 1
            if cell_bytes > MAX_TEXT_CELL_BYTES:
                raise ValueError(f"{source}: string exceeds {MAX_TEXT_CELL_BYTES}-byte limit")
            text_bytes += cell_bytes
            if text_bytes > MAX_TOTAL_TEXT_BYTES:
                raise ValueError(f"{source}: text exceeds {MAX_TOTAL_TEXT_BYTES}-byte limit")
            continue
        if byte in (ord('{'), ord('[')):
            depth += 1
            nodes += 1
            if depth > MAX_JSON_DEPTH:
                raise ValueError(f"{source}: JSON nesting exceeds {MAX_JSON_DEPTH}-level limit")
        elif byte in (ord('}'), ord(']')):
            depth -= 1
        elif byte not in b" \t\r\n,:":
            nodes += 1
            if nodes > MAX_JSON_NODES:
                raise ValueError(f"{source}: JSON nodes exceed {MAX_JSON_NODES}-node limit")
            while index < len(raw) and raw[index] not in b" \t\r\n,:]}":
                index += 1
            continue
        if nodes > MAX_JSON_NODES:
            raise ValueError(f"{source}: JSON nodes exceed {MAX_JSON_NODES}-node limit")
        index += 1


def _bounded_read(path):
    source = str(path)
    # read(limit + 1) makes the cap effective even if the file changes after
    # metadata is read, and avoids an unbounded read_text allocation.
    with path.open("rb") as stream:
        raw = stream.read(MAX_LOG_BYTES + 1)
    if len(raw) > MAX_LOG_BYTES:
        raise ValueError(f"{source}: input exceeds {MAX_LOG_BYTES}-byte limit")
    _preflight_json(raw, source)
    try:
        return json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as error:
        raise ValueError(f"{source}: input is not valid UTF-8") from error


def load_log(path=LOG):
    data = _bounded_read(path)
    if not isinstance(data, dict):
        raise ValueError("hazard log must be a JSON object")
    if data.get("schema") != "openatca.hazard-log":
        raise ValueError("unexpected hazard log schema")
    if data.get("assessment_status") != "DRAFT_NOT_ASSESSED":
        raise ValueError("unexpected assessment status")
    if data.get("coverage_status") != "DECLARED_FUNCTION_SCOPE_ONLY":
        raise ValueError("unexpected coverage status")
    return data


def records(data, source="hazard log"):
    # Check declared collection sizes before sorting, building indexes, or
    # entering loops. Each cap is sized from the current register above.
    hazards_input = data["hazards"]
    requirement_ids = data["safety_requirement_ids"]
    assessments_input = data["safety_requirement_assessments"]
    if not isinstance(hazards_input, list) or len(hazards_input) > MAX_HAZARDS:
        raise ValueError(f"{source}: hazards exceed {MAX_HAZARDS}-item limit")
    if not isinstance(requirement_ids, list) or len(requirement_ids) > MAX_REQUIREMENTS:
        raise ValueError(f"{source}: requirements exceed {MAX_REQUIREMENTS}-item limit")
    if not isinstance(assessments_input, list) or len(assessments_input) > MAX_REQUIREMENTS:
        raise ValueError(f"{source}: assessments exceed {MAX_REQUIREMENTS}-item limit")

    reference_count = 0
    mitigation_count = 0
    for hazard in hazards_input:
        if not isinstance(hazard, dict):
            raise ValueError("hazard entries must be objects")
        for kind in ("verification_refs", "proof_refs", "mitigation_requirement_ids"):
            values = hazard.get(kind, [])
            if not isinstance(values, list):
                raise ValueError(f"{hazard.get('id', 'hazard')}: {kind} must be a list")
            if kind == "mitigation_requirement_ids":
                mitigation_count += len(values)
            else:
                reference_count += len(values)
    if reference_count > MAX_REFERENCES:
        raise ValueError(f"{source}: references exceed {MAX_REFERENCES}-item limit")
    if mitigation_count > MAX_MITIGATIONS:
        raise ValueError(f"{source}: mitigation links exceed {MAX_MITIGATIONS}-item limit")
    # A conservative upper bound on visits in validation, indexing and output
    # assembly. Sorting is limited to the already-capped requirement/ref lists.
    work_units = (len(hazards_input) * 8 + len(requirement_ids) * 4
                  + len(assessments_input) * 2 + reference_count * 4
                  + mitigation_count * 2)
    if work_units > MAX_WORK_UNITS:
        raise ValueError(f"{source}: register work exceeds {MAX_WORK_UNITS}-unit limit")

    hazards = sorted(hazards_input, key=lambda row: row["id"])
    assessments = {row["id"]: row for row in assessments_input}
    hazards_by_id = {row["id"]: row for row in hazards}
    if len(assessments) != len(requirement_ids):
        raise ValueError("safety requirement assessment/register count mismatch")
    if set(assessments) != set(requirement_ids):
        raise ValueError("safety requirement assessment/register IDs mismatch")
    refs_by_requirement = {}
    for hazard in hazards:
        for kind in ("verification_refs", "proof_refs"):
            for ref in hazard.get(kind, []):
                req_id = ref["requirement_id"]
                if req_id not in assessments or assessments[req_id]["hazard_id"] != hazard["id"]:
                    raise ValueError(f"{req_id}: {kind} hazard reference does not match its assessment")
                refs_by_requirement.setdefault((hazard["id"], req_id, kind), []).append(ref)
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

    for req_id in sorted(requirement_ids):
        assessment = assessments[req_id]
        hazard = hazards_by_id.get(assessment["hazard_id"])
        if hazard is None:
            raise ValueError(f"{req_id}: assessment references unknown hazard {assessment['hazard_id']}")
        verifications = refs_by_requirement.get((hazard["id"], req_id, "verification_refs"), [])
        proofs = refs_by_requirement.get((hazard["id"], req_id, "proof_refs"), [])
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


def _estimated_artifact_bytes(rows):
    """Conservative output bound checked before rendering any table strings."""
    columns = list(rows[0])
    values = [str(value) for row in rows for value in row.values()]
    values.extend(columns)
    byte_count = sum(len(value.encode("utf-8")) for value in values)
    cell_count = len(values)
    row_count = len(rows) + 1  # Include the header in both formats.
    column_count = len(columns)
    csv_bound = 2 * byte_count + 2 * cell_count + row_count * (column_count + 1)
    markdown_bound = (4 * byte_count + (row_count + 1) * (3 * column_count + 3)
                      + 2 * MAX_TEXT_CELL_BYTES + 512)
    return csv_bound + markdown_bound


def generate(check=False, log_path=LOG, out=OUT):
    data = load_log(log_path)
    artifacts = records(data, source=log_path)
    estimated_bytes = sum(_estimated_artifact_bytes(rows) for _, rows in artifacts)
    if estimated_bytes > MAX_GENERATED_BYTES:
        raise ValueError(f"estimated generated tables exceed {MAX_GENERATED_BYTES}-byte limit")
    staged = []
    generated_bytes = 0
    for name, rows in artifacts:
        for ext, content in (("md", render_markdown(rows, data)), ("csv", render_csv(rows))):
            generated_bytes += len(content.encode("utf-8"))
            if generated_bytes > MAX_GENERATED_BYTES:
                raise ValueError(f"generated tables exceed {MAX_GENERATED_BYTES}-byte limit")
            staged.append((name, ext, content))

    # Input, work and output checks all complete before creating or changing
    # any output path, so rejected inputs cannot leave partial generated files.
    out.mkdir(parents=True, exist_ok=True)
    stale = []
    for name, ext, content in staged:
        path = out / f"{name}.{ext}"
        if check:
            if not path.is_file() or path.stat().st_size > MAX_GENERATED_BYTES:
                stale.append(str(path.relative_to(ROOT)))
            elif path.open(encoding="utf-8", newline="").read(MAX_GENERATED_BYTES + 1) != content:
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
