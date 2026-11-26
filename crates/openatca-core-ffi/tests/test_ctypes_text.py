"""Cross-language checks for the small CC0 synthetic text-parser vector table."""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "python"))

from openatca import OpenAtcaError, load  # noqa: E402


def _hex(value: str) -> bytes:
    return b"" if value == "-" else bytes.fromhex(value)


def run(library_path: str, vector_path: str) -> int:
    api = load(library_path)
    rows = Path(vector_path).read_text(encoding="ascii").splitlines()
    if not rows or rows[0] != "module\tcase\tinput_hex\tstatus\tkind\tposition\tcounts\tpool_hex":
        raise AssertionError("text vector header mismatch")
    seen = 0
    for line in rows[1:]:
        module, case, input_hex, status_text, kind_text, pos_text, counts_text, pool_hex = line.split("\t")
        expected_status, expected_kind, expected_position = int(status_text), int(kind_text), int(pos_text)
        data = _hex(input_hex)
        if expected_status:
            try:
                if module == "json": api.text.parse_json(data)
                elif module == "xml": api.text.parse_xml(data)
                elif module == "csv": api.text.parse_csv(data)
                else: raise AssertionError(f"unknown module {module}")
            except OpenAtcaError as error:
                if (error.code, error.parser_kind, error.parser_position) != (
                    expected_status, expected_kind, expected_position
                ):
                    raise AssertionError(f"{case}: parser refusal mismatch") from error
            else:
                raise AssertionError(f"{case}: malformed vector was accepted")
        else:
            wanted_counts = tuple(int(item) for item in counts_text.split(","))
            wanted_pool = _hex(pool_hex)
            if module == "json":
                result = api.text.parse_json(data)
                actual_counts = (len(result.nodes), len(result.value_pool))
                if not result.nodes or result.nodes[0].kind != 7:
                    raise AssertionError(f"{case}: JSON root mismatch")
                pool = result.value_pool
            elif module == "xml":
                result = api.text.parse_xml(data)
                actual_counts = (len(result.elements), len(result.attributes), len(result.texts),
                                 len(result.namespaces), len(result.namespace_uri_pool))
                if not result.texts:
                    raise AssertionError(f"{case}: XML text node missing")
                node = result.texts[0]
                pool = result.namespace_uri_pool
                if api.text.decode_xml_span(data, node.raw_value.start, node.raw_value.length, 1) != b"v&":
                    raise AssertionError(f"{case}: XML span decode mismatch")
            elif module == "csv":
                result = api.text.parse_csv(data)
                actual_counts = (len(result.rows), len(result.fields), len(result.value_pool))
                if not result.rows or result.rows[0].physical_line != 1:
                    raise AssertionError(f"{case}: CSV header row missing")
                pool = result.value_pool
            else:
                raise AssertionError(f"unknown module {module}")
            if actual_counts != wanted_counts or pool != wanted_pool:
                raise AssertionError(f"{case}: result differs from language-neutral vector")
        seen += 1
    if seen != 6:
        raise AssertionError(f"expected 6 text vectors, found {seen}")
    return seen


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: test_ctypes_text.py /absolute/path/to/library text-v1.tsv")
    print(f"text vectors passed: {run(sys.argv[1], sys.argv[2])}")
