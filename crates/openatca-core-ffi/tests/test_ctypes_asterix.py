"""ctypes ASTERIX vectors use the same CC0 TSV as the Rust and C checks."""
from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "python"))
from openatca import AsterixError, _MAX_ASTERIX_RECORDS, load  # noqa: E402

assert _MAX_ASTERIX_RECORDS == 21_844


def run(library_path: str, vector_path: str) -> int:
    api = load(library_path).asterix
    rows = Path(vector_path).read_text(encoding="ascii").splitlines()
    if not rows or rows[0] != (
        "case\tselector\thex\tstatus\tkind\tcategory\trecords\tconsumed\t"
        "record\titem\tsubitem\toffset"
    ):
        raise AssertionError("ASTERIX vector header mismatch")
    seen = 0
    for line in rows[1:]:
        (case, selector, encoded, status, kind, category, count, consumed,
         record, item, subitem, offset) = line.split("\t")
        data = bytes.fromhex(encoded)
        status, kind, category = int(status), int(kind), int(category)
        count, consumed = int(count), int(consumed)
        if status:
            try:
                api.decode_block(data, cat021_edition=int(selector))
            except AsterixError as error:
                actual = (error.code, error.asterix_kind, error.category, error.record,
                          error.item, error.subitem, error.offset)
                expected = (status, kind, category, int(record), int(item), int(subitem), int(offset))
                if actual != expected:
                    raise AssertionError(f"{case}: detailed refusal differs: {actual} != {expected}") from error
            else:
                raise AssertionError(f"{case}: malformed vector accepted")
        else:
            result = api.decode_block(data, cat021_edition=int(selector))
            if (result.category, result.consumed_bytes, len(result.records)) != (category, consumed, count):
                raise AssertionError(f"{case}: block metadata mismatch")
            if result.records:
                first = result.records[0]
                if first.category != category or not first.present_frn_mask or not first.present_field_mask:
                    raise AssertionError(f"{case}: projection/presence mismatch")
                if first.raw_untyped_frn_mask & ~first.present_frn_mask:
                    raise AssertionError(f"{case}: raw-untyped mask is not a subset of present items")
                if first.target_address == 0:
                    raise AssertionError(f"{case}: address projection missing")
                if category == 21 and not any(position.item == 131 for position in first.positions):
                    raise AssertionError(f"{case}: CAT021 high-resolution position missing")
                if category == 48 and not first.positions:
                    raise AssertionError(f"{case}: CAT048 position missing")
                if category == 62 and len(first.vertical) != 4:
                    raise AssertionError(f"{case}: CAT062 vertical values missing")
        seen += 1
    if seen != 9:
        raise AssertionError(f"expected 9 ASTERIX vectors, found {seen}")
    return seen


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: test_ctypes_asterix.py /absolute/path/to/library asterix-v1.tsv")
    print(f"ASTERIX ctypes vectors passed: {run(sys.argv[1], sys.argv[2])}")
