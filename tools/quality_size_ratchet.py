#!/usr/bin/env python3
"""Bounded source-size and exact-duplicate ratchet for tracked Rust and Python."""

from __future__ import annotations

import argparse
import ast
from array import array
import hashlib
import io
import json
import os
import stat
import subprocess
import sys
import tokenize
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Iterator


ALGORITHM = "tracked-rust-python-fn-v2"
SCOPE = "all git-tracked .rs and .py source files; no path exclusions"
WINDOW_TOKENS = 24
MIN_IDENTIFIERS = 6
MAX_SOURCE_FILES = 100_000
MAX_SOURCE_BYTES = 128 * 1024 * 1024
MAX_SOURCE_FILE_BYTES = 8 * 1024 * 1024
MAX_BASELINE_BYTES = 64 * 1024 * 1024
MAX_CODE_TOKENS = 4_000_000
# Every qualifying window consumes at least one body token, so the 4M token cap bounds this
# two-pass scan without a smaller source-content-dependent rejection threshold.
MAX_DUPLICATE_WINDOWS = MAX_CODE_TOKENS
# Detailed fingerprints and locations use bounded storage; excess groups fail closed.
MAX_DUPLICATE_BLOCK_GROUPS = 100_000
PACKED_LOCATION_PART_MAX = (1 << 32) - 1
MAX_DUPLICATE_COMPARISONS = 25_000_000
MAX_LEX_TOKENS_PER_FILE = 1_000_000
BASELINE_PATH = Path("tools/quality-size-baseline.json")
WORKSPACE_BINDING = "openatca-rust-workspace-v1"
WORKSPACE_MARKERS = (
    "Cargo.toml", "docs/RULESET.md", "crates/oa-track/Cargo.toml",
    "tools/oa-trace/Cargo.toml", "tools/rust_proofs.json",
)


class RatchetError(Exception):
    pass


@dataclass
class WorkBudget:
    remaining: int

    def reserve(self, amount: int) -> None:
        if amount < 0 or amount > self.remaining:
            raise RatchetError("duplicate comparison work limit exceeded")
        self.remaining -= amount


@dataclass(frozen=True)
class Token:
    kind: str
    value: str
    line: int
    column: int = 0


@dataclass(frozen=True)
class Function:
    path: str
    name: str
    start_line: int
    end_line: int
    argument_count: int
    body: tuple[Token, ...]


def _rust_tokens(text: str) -> list[Token]:
    """Lex enough Rust syntax for item boundaries; comments are discarded, literals retained."""
    result: list[Token] = []
    i = 0
    line = 1
    line_start = 0
    n = len(text)

    def advance(end: int) -> None:
        nonlocal i, line, line_start
        segment = text[i:end]
        line += segment.count("\n")
        last_newline = segment.rfind("\n")
        if last_newline >= 0:
            line_start = i + last_newline + 1
        i = end

    def emit(item: Token) -> None:
        if len(result) >= MAX_LEX_TOKENS_PER_FILE:
            raise RatchetError("per-file lexer token limit exceeded")
        result.append(item)

    while i < n:
        char = text[i]
        if char.isspace():
            advance(i + 1)
            continue
        if text.startswith("//", i):
            end = text.find("\n", i)
            advance(n if end < 0 else end)
            continue
        if text.startswith("/*", i):
            depth = 1
            cursor = i + 2
            while cursor < n and depth:
                if text.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif text.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            if depth:
                raise RatchetError("unterminated Rust block comment")
            advance(cursor)
            continue

        start_line = line
        start_column = i - line_start
        raw_prefix = None
        if text.startswith('br', i) or text.startswith('r', i):
            raw_prefix = "br" if text.startswith("br", i) else "r"
        if raw_prefix is not None:
            quote_at = i + len(raw_prefix)
            hashes = 0
            while quote_at + hashes < n and text[quote_at + hashes] == "#":
                hashes += 1
            quote_at += hashes
            if quote_at < n and text[quote_at] == '"':
                closing = '"' + ("#" * hashes)
                end = text.find(closing, quote_at + 1)
                if end < 0:
                    raise RatchetError("unterminated Rust raw string")
                end += len(closing)
                emit(Token("literal", text[i:end], start_line, start_column))
                advance(end)
                continue

        if char == '"' or char == "'":
            # Apostrophes introducing lifetimes are punctuation; quoted chars are literals.
            if char == "'":
                cursor = i + 1
                if cursor < n and text[cursor] == "\\":
                    cursor += 2
                else:
                    cursor += 1
                if cursor >= n or text[cursor] != "'":
                    emit(Token("punct", char, start_line, start_column))
                    advance(i + 1)
                    continue
            cursor = i + 1
            escaped = False
            while cursor < n:
                current = text[cursor]
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == char:
                    cursor += 1
                    break
                cursor += 1
            else:
                raise RatchetError("unterminated Rust literal")
            emit(Token("literal", text[i:cursor], start_line, start_column))
            advance(cursor)
            continue

        if char == "r" and text.startswith("r#", i) and i + 2 < n and _ident_start(text[i + 2]):
            cursor = i + 3
            while cursor < n and _ident_continue(text[cursor]):
                cursor += 1
            emit(Token("identifier", text[i:cursor], start_line, start_column))
            advance(cursor)
            continue
        if _ident_start(char):
            cursor = i + 1
            while cursor < n and _ident_continue(text[cursor]):
                cursor += 1
            emit(Token("identifier", text[i:cursor], start_line, start_column))
            advance(cursor)
            continue
        if char.isdigit():
            cursor = i + 1
            while cursor < n and (text[cursor].isalnum() or text[cursor] == "_"):
                cursor += 1
            emit(Token("number", text[i:cursor], start_line, start_column))
            advance(cursor)
            continue
        operator = next((candidate for candidate in (
            "->", "=>", "::", "..=", "..", "<=", ">=", "==", "!=", "&&", "||"
        ) if text.startswith(candidate, i)), None)
        if operator is not None:
            emit(Token("operator", operator, start_line, start_column))
            advance(i + len(operator))
        else:
            emit(Token("punct", char, start_line, start_column))
            advance(i + 1)
    return result


def _ident_start(char: str) -> bool:
    return char == "_" or char.isalpha()


def _ident_continue(char: str) -> bool:
    return char == "_" or char.isalnum()


_ITEM_PREFIXES = {"pub", "async", "unsafe", "const", "extern", "default", "auto"}
_ITEM_BOUNDARIES = {"{", "}", ";", "]"}


def _is_rust_item_fn(tokens: list[Token], index: int) -> bool:
    previous = index - 1
    if previous >= 0 and tokens[previous].value == ")":
        depth = 1
        previous -= 1
        while previous >= 0 and depth:
            if tokens[previous].value == ")":
                depth += 1
            elif tokens[previous].value == "(":
                depth -= 1
            previous -= 1
        visibility = previous
        if visibility >= 0 and tokens[visibility].value == "pub":
            previous = visibility - 1
    if previous >= 0 and tokens[previous].kind == "literal" and previous > 0 and tokens[previous - 1].value == "extern":
        previous -= 1
    while previous >= 0 and tokens[previous].value in _ITEM_PREFIXES:
        previous -= 1
    return previous < 0 or tokens[previous].value in _ITEM_BOUNDARIES


def _matching(tokens: list[Token], opening: int, left: str, right: str) -> int | None:
    depth = 0
    for index in range(opening, len(tokens)):
        value = tokens[index].value
        if value == left:
            depth += 1
        elif value == right:
            depth -= 1
            if depth == 0:
                return index
    return None


def _matching_generics(tokens: list[Token], opening: int) -> int | None:
    depth = 0
    brace_depth = 0
    for index in range(opening, len(tokens)):
        value = tokens[index].value
        if value == "{":
            brace_depth += 1
        elif value == "}":
            brace_depth -= 1
        elif brace_depth == 0 and value == "<":
            depth += 1
        elif brace_depth == 0 and value == ">":
            depth -= 1
            if depth == 0:
                return index
    return None


def _rust_arg_count(tokens: list[Token], start: int, end: int) -> int:
    if start + 1 == end:
        return 0
    counts = {"(": 0, "[": 0, "<": 0}
    matching = {")": "(", "]": "[", "}": "{", ">": "<"}
    arguments = 0
    has_token = False
    brace_depth = 0
    for token in tokens[start + 1:end]:
        value = token.value
        if value == "," and brace_depth == 0 and not any(counts.values()):
            if has_token:
                arguments += 1
                has_token = False
            continue
        if value == "{" and brace_depth == 0:
            brace_depth = 1
        elif value == "}" and brace_depth:
            brace_depth = 0
        elif brace_depth == 0 and value in counts:
            counts[value] += 1
        elif brace_depth == 0 and value in matching and matching[value] in counts and counts[matching[value]]:
            counts[matching[value]] -= 1
        has_token = True
    return arguments + int(has_token)


def _rust_functions(path: str, text: str, tokens: list[Token]) -> list[Function]:
    functions: list[Function] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.value == "macro_rules" and index + 1 < len(tokens) and tokens[index + 1].value == "!":
            opening = next((cursor for cursor in range(index + 2, len(tokens))
                            if tokens[cursor].value == "{"), None)
            if opening is not None:
                closing = _matching(tokens, opening, "{", "}")
                if closing is not None:
                    index = closing + 1
                    continue
        if token.value != "fn" or not _is_rust_item_fn(tokens, index):
            # Skip token trees emitted by macro invocations so DSL keywords aren't parsed as items.
            if token.value == "!" and index + 1 < len(tokens) and tokens[index + 1].value in ("(", "[", "{"):
                opening = index + 1
                closing = _matching(tokens, opening, tokens[opening].value,
                                     {"(": ")", "[": "]", "{": "}"}[tokens[opening].value])
                if closing is not None:
                    index = closing + 1
                    continue
            index += 1
            continue
        if index + 1 >= len(tokens) or tokens[index + 1].kind != "identifier":
            raise RatchetError(f"{path}:{token.line}: malformed Rust function item")
        name = tokens[index + 1].value.removeprefix("r#")
        cursor = index + 2
        if cursor < len(tokens) and tokens[cursor].value == "<":
            generic_close = _matching_generics(tokens, cursor)
            if generic_close is None:
                raise RatchetError(f"{path}:{token.line}: unterminated function generics")
            cursor = generic_close + 1
        opening_paren = next((position for position in range(cursor, len(tokens))
                              if tokens[position].value in ("(", "{", ";")), None)
        if opening_paren is None or tokens[opening_paren].value != "(":
            raise RatchetError(f"{path}:{token.line}: missing Rust function argument list")
        closing_paren = _matching(tokens, opening_paren, "(", ")")
        if closing_paren is None:
            raise RatchetError(f"{path}:{token.line}: unterminated function arguments")
        body_open = None
        declaration_end = None
        nesting = {"(": 0, "[": 0, "<": 0}
        closes = {")": "(", "]": "[", ">": "<"}
        for cursor in range(closing_paren + 1, len(tokens)):
            value = tokens[cursor].value
            if value == "{" and not any(nesting.values()):
                body_open = cursor
                break
            if value in nesting:
                nesting[value] += 1
            elif value in closes and nesting[closes[value]]:
                nesting[closes[value]] -= 1
            elif value == ";" and not any(nesting.values()):
                declaration_end = cursor
                break
        if body_open is None and declaration_end is None:
            raise RatchetError(f"{path}:{token.line}: Rust function has no body or declaration terminator")
        body_close = None if body_open is None else _matching(tokens, body_open, "{", "}")
        if body_open is not None and body_close is None:
            raise RatchetError(f"{path}:{token.line}: unterminated function body")
        end_index = body_close if body_close is not None else declaration_end
        assert end_index is not None
        body = () if body_open is None else tuple(tokens[body_open + 1:body_close])
        functions.append(Function(
            path=path,
            name=name,
            start_line=token.line,
            end_line=tokens[end_index].line,
            argument_count=_rust_arg_count(tokens, opening_paren, closing_paren),
            body=body,
        ))
        index += 1
    return functions


def _python_functions(path: str, text: str) -> list[Function]:
    try:
        tree = ast.parse(text, filename=path)
        lexical = tokenize.generate_tokens(io.StringIO(text).readline)
    except (SyntaxError, tokenize.TokenError, StopIteration) as error:
        raise RatchetError(f"{path}: Python syntax/tokenization failed: {error}") from error
    ignored = {tokenize.ENCODING, tokenize.NL, tokenize.NEWLINE, tokenize.INDENT,
               tokenize.DEDENT, tokenize.COMMENT, tokenize.ENDMARKER}
    all_tokens: list[Token] = []
    try:
        for item in lexical:
            if item.type in ignored:
                continue
            if len(all_tokens) >= MAX_LEX_TOKENS_PER_FILE:
                raise RatchetError("per-file lexer token limit exceeded")
            all_tokens.append(Token(
                "identifier" if item.type == tokenize.NAME else str(item.type),
                item.string,
                item.start[0],
                item.start[1],
            ))
    except (tokenize.TokenError, StopIteration) as error:
        raise RatchetError(f"{path}: Python tokenization failed: {error}") from error
    functions: list[Function] = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        end_line = node.end_lineno or node.lineno
        all_args = (node.args.posonlyargs + node.args.args + node.args.kwonlyargs)
        argument_count = len(all_args) + int(node.args.vararg is not None) + int(node.args.kwarg is not None)
        if not node.body:
            body_tokens: tuple[Token, ...] = ()
        else:
            first_statement = node.body[0]
            body_start = (first_statement.lineno, first_statement.col_offset)
            body_tokens = tuple(
                item for item in all_tokens
                if (item.line, item.column) >= body_start
                and item.line <= end_line
            )
        decorators = [item.lineno for item in node.decorator_list]
        start_line = min([node.lineno, *decorators])
        functions.append(Function(path, node.name, start_line, end_line, argument_count, body_tokens))
    return functions


def _function_tokens(path: str, text: str) -> list[Function]:
    if path.endswith(".py"):
        return _python_functions(path, text)
    tokens = _rust_tokens(text)
    return _rust_functions(path, text, tokens)


def _read_tracked_sources(root: Path) -> Iterator[tuple[str, str]]:
    process = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.rs", "*.py"],
        cwd=root, check=True, capture_output=True,
    )
    paths = sorted(item.decode("utf-8") for item in process.stdout.split(b"\0") if item)
    if len(paths) > MAX_SOURCE_FILES:
        raise RatchetError("tracked source file limit exceeded")
    total_bytes = 0
    for relative in paths:
        source = root / relative
        before = source.stat(follow_symlinks=False)
        if not stat.S_ISREG(before.st_mode) or source.is_symlink():
            raise RatchetError(f"tracked source is not a regular file: {relative}")
        remaining = MAX_SOURCE_BYTES - total_bytes
        if before.st_size < 0 or before.st_size > MAX_SOURCE_FILE_BYTES or before.st_size > remaining:
            raise RatchetError("tracked source byte limit exceeded")
        with source.open("rb") as stream:
            payload = stream.read(remaining + 1)
            after = os.fstat(stream.fileno())
        if (len(payload) != before.st_size or
                (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) !=
                (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
            raise RatchetError(f"tracked source changed while reading: {relative}")
        total_bytes += len(payload)
        try:
            text = payload.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RatchetError(f"tracked source is not UTF-8: {relative}") from error
        yield relative, text


def _intern_function_bodies(
    functions: list[Function],
) -> tuple[list[array], list[bytearray]]:
    token_total = sum(len(function.body) for function in functions)
    if token_total > MAX_CODE_TOKENS:
        raise RatchetError("code token limit exceeded")
    intern: dict[tuple[str, str, str], int] = {}
    function_ids: list[array] = []
    function_identifier: list[bytearray] = []
    for function in functions:
        language = "rust" if function.path.endswith(".rs") else "python"
        ids = array("Q")
        identifiers = bytearray()
        for token in function.body:
            key = (language, token.kind, token.value)
            token_id = intern.get(key)
            if token_id is None:
                token_id = len(intern) + 1
                intern[key] = token_id
            ids.append(token_id)
            identifiers.append(token.kind == "identifier")
        function_ids.append(ids)
        function_identifier.append(identifiers)
    return function_ids, function_identifier


def _index_duplicate_windows(
    function_ids: list[array], function_identifier: list[bytearray],
) -> dict[int, array]:
    """Count bounded rolling hashes, then retain locations only for repeated hashes."""
    counts: Counter[int] = Counter()
    qualifying = 0
    for _, _, rolling in _qualified_windows(function_ids, function_identifier):
        qualifying += 1
        if qualifying > MAX_DUPLICATE_WINDOWS:
            raise RatchetError("qualifying duplicate-window limit exceeded")
        counts[rolling] += 1
    repeated = {fingerprint: array("Q") for fingerprint, count in counts.items() if count > 1}
    for function_index, start, rolling in _qualified_windows(function_ids, function_identifier):
        locations = repeated.get(rolling)
        if locations is not None:
            locations.append(_encode_location(function_index, start))
    return repeated


def _encode_location(function_index: int, start: int) -> int:
    if not (0 <= function_index <= PACKED_LOCATION_PART_MAX and
            0 <= start <= PACKED_LOCATION_PART_MAX):
        raise RatchetError("duplicate source location exceeds packed representation")
    return (function_index << 32) | start


def _qualified_windows(
    function_ids: list[array], function_identifier: list[bytearray],
) -> Iterator[tuple[int, int, int]]:
    base = 1_099_511_628_211
    mask = (1 << 64) - 1
    remove_power = pow(base, WINDOW_TOKENS - 1, 1 << 64)
    for function_index, ids in enumerate(function_ids):
        if len(ids) < WINDOW_TOKENS:
            continue
        ident_counts: Counter[int] = Counter(
            ids[index] for index in range(WINDOW_TOKENS)
            if function_identifier[function_index][index]
        )
        rolling = 0
        for token_id in ids[:WINDOW_TOKENS]:
            rolling = (rolling * base + token_id) & mask
        for start in range(len(ids) - WINDOW_TOKENS + 1):
            if len(ident_counts) >= MIN_IDENTIFIERS:
                yield function_index, start, rolling
            next_start = start + 1
            if next_start + WINDOW_TOKENS > len(ids):
                continue
            old_index = start
            new_index = next_start + WINDOW_TOKENS - 1
            if function_identifier[function_index][old_index]:
                old_id = ids[old_index]
                ident_counts[old_id] -= 1
                if ident_counts[old_id] == 0:
                    del ident_counts[old_id]
            if function_identifier[function_index][new_index]:
                ident_counts[ids[new_index]] += 1
            rolling = ((rolling - ids[old_index] * remove_power) * base + ids[new_index]) & mask


def _expand_duplicate_windows(
    functions: list[Function],
    function_ids: list[array],
    window_index: dict[int, array],
) -> dict[str, dict[str, object]]:
    blocks: dict[str, dict[str, object]] = {}
    covered: dict[tuple[int, int], list[tuple[int, int, int, int]]] = defaultdict(list)
    work = WorkBudget(MAX_DUPLICATE_COMPARISONS)
    for locations in window_index.values():
        # Resolve hash collisions and count each source window once even when an enclosing
        # function body also contains the same nested-function tokens.
        exact_groups: dict[tuple[int, ...], list[tuple[int, int]]] = defaultdict(list)
        for encoded in locations:
            function_index, start = encoded >> 32, encoded & PACKED_LOCATION_PART_MAX
            exact_groups[tuple(function_ids[function_index][start:start + WINDOW_TOKENS])].append(
                (function_index, start)
            )
        for group in exact_groups.values():
            physical: dict[tuple[str, int, int], tuple[int, int]] = {}
            for function_index, start in group:
                function = functions[function_index]
                first_token = function.body[start]
                physical.setdefault((function.path, first_token.line, first_token.column),
                                    (function_index, start))
            group = list(physical.values())
            if len(group) < 2:
                continue
            base_function, base_start = group[0]
            for other_function, other_start in group[1:]:
                work.reserve(1)
                left_start = base_start
                right_start = other_start
                pair = (base_function, other_function)
                if base_function > other_function:
                    pair = (other_function, base_function)
                    left_start, right_start = other_start, base_start
                intervals = covered[pair]
                work.reserve(len(intervals))
                already_covered = False
                for start_a, end_a, start_b, end_b in intervals:
                    if (start_a <= left_start < end_a and start_b <= right_start < end_b
                            and left_start - start_a == right_start - start_b):
                        already_covered = True
                        break
                if already_covered:
                    continue
                source_left, source_right = pair
                source_start_left, source_start_right = left_start, right_start
                left_ids = function_ids[source_left]
                right_ids = function_ids[source_right]
                left_body = functions[source_left].body
                right_body = functions[source_right].body
                left = source_start_left
                right = source_start_right
                while left > 0 and right > 0:
                    work.reserve(1)
                    if left_ids[left - 1] != right_ids[right - 1]:
                        break
                    left -= 1
                    right -= 1
                end_left = source_start_left + WINDOW_TOKENS
                end_right = source_start_right + WINDOW_TOKENS
                while end_left < len(left_ids) and end_right < len(right_ids):
                    work.reserve(1)
                    if left_ids[end_left] != right_ids[end_right]:
                        break
                    end_left += 1
                    end_right += 1
                block_ids = left_ids[left:end_left]
                if len(block_ids) < WINDOW_TOKENS:
                    continue
                block_tokens = left_body[left:end_left]
                distinct = {token.value for token in block_tokens if token.kind == "identifier"}
                if len(distinct) < MIN_IDENTIFIERS:
                    continue
                covered[pair].append((left, end_left, right, end_right))
                language = "rust" if functions[source_left].path.endswith(".rs") else "python"
                canonical = [f"{token.kind}:{token.value}" for token in block_tokens]
                fingerprint = hashlib.sha256(
                    (language + "\n" + "\n".join(canonical)).encode("utf-8")
                ).hexdigest()
                row = {
                    "path": functions[source_left].path,
                    "function": functions[source_left].name,
                    "start_line": block_tokens[0].line,
                    "start_column": block_tokens[0].column,
                    "end_line": block_tokens[-1].line,
                    "end_column": block_tokens[-1].column,
                    "token_count": len(block_tokens),
                }
                other_row = {
                    "path": functions[source_right].path,
                    "function": functions[source_right].name,
                    "start_line": right_body[right].line,
                    "start_column": right_body[right].column,
                    "end_line": right_body[end_right - 1].line,
                    "end_column": right_body[end_right - 1].column,
                    "token_count": len(block_tokens),
                }
                entry = blocks.setdefault(fingerprint, {
                    "token_count": len(block_tokens),
                    "token_window": canonical[:12],
                    "occurrences": {},
                })
                if len(blocks) > MAX_DUPLICATE_BLOCK_GROUPS:
                    raise RatchetError("duplicate block group limit exceeded")
                occur = entry["occurrences"]
                occur[(row["path"], row["function"], row["start_line"], row["start_column"])] = row
                occur[(other_row["path"], other_row["function"], other_row["start_line"],
                       other_row["start_column"])] = other_row
    return blocks


def _duplicate_inventory(functions: list[Function]) -> tuple[dict[str, int], dict[str, dict[str, object]]]:
    """Find maximal repeated 24-token runs, verifying rolling-hash matches exactly."""
    function_ids, function_identifier = _intern_function_bodies(functions)
    window_index = _index_duplicate_windows(function_ids, function_identifier)
    blocks = _expand_duplicate_windows(functions, function_ids, window_index)
    frequencies: dict[str, int] = {}
    details: dict[str, dict[str, object]] = {}
    for fingerprint, block in blocks.items():
        rows = sorted(block["occurrences"].values(),
                      key=lambda row: (row["path"], row["start_line"], row["start_column"], row["function"]))
        if len(rows) < 2:
            continue
        frequencies[fingerprint] = len(rows) - 1
        details[fingerprint] = {
            "token_count": block["token_count"],
            "token_window": block["token_window"],
            "occurrences": rows,
        }
    return frequencies, details


def analyze_sources(sources: Iterable[tuple[str, str]]) -> dict[str, object]:
    functions: list[Function] = []
    files = 0
    body_tokens = 0
    for path, text in sources:
        files += 1
        parsed = _function_tokens(path, text)
        body_tokens += sum(len(function.body) for function in parsed)
        if body_tokens > MAX_CODE_TOKENS:
            raise RatchetError("code token limit exceeded")
        functions.extend(parsed)
    duplicates, details = _duplicate_inventory(functions)
    long_functions = [
        {"path": f.path, "name": f.name, "start_line": f.start_line,
         "end_line": f.end_line, "lines": f.end_line - f.start_line + 1}
        for f in functions if f.end_line - f.start_line + 1 > 150
    ]
    wide_functions = [
        {"path": f.path, "name": f.name, "line": f.start_line,
         "arguments": f.argument_count}
        for f in functions if f.argument_count > 7
    ]
    return {
        "tracked_source_files": files,
        "function_count": len(functions),
        "long_function_count": len(long_functions),
        "long_functions": long_functions,
        "wide_argument_function_count": len(wide_functions),
        "wide_argument_functions": wide_functions,
        "duplicate_block_count": sum(duplicates.values()),
        "duplicate_frequencies": duplicates,
        "duplicate_details": details,
    }


def build_baseline(root: Path) -> dict[str, object]:
    root = _validate_workspace(root)
    summary = analyze_sources(_read_tracked_sources(root))
    baseline_summary = dict(summary)
    baseline_summary.pop("duplicate_details", None)
    return {
        "schema": 1,
        "algorithm": ALGORITHM,
        "scope": SCOPE,
        "workspace_binding": WORKSPACE_BINDING,
        "limits": {
            "long_function_lines_exceeds": 150,
            "argument_count_exceeds": 7,
            "duplicate_window_tokens": WINDOW_TOKENS,
            "duplicate_minimum_distinct_identifiers": MIN_IDENTIFIERS,
            "tracked_source_files_max": MAX_SOURCE_FILES,
            "tracked_source_bytes_max": MAX_SOURCE_BYTES,
            "tracked_source_file_bytes_max": MAX_SOURCE_FILE_BYTES,
            "baseline_bytes_max": MAX_BASELINE_BYTES,
            "function_body_tokens_max": MAX_CODE_TOKENS,
            "duplicate_windows_max": MAX_DUPLICATE_WINDOWS,
            "duplicate_block_groups_max": MAX_DUPLICATE_BLOCK_GROUPS,
            "duplicate_comparisons_max": MAX_DUPLICATE_COMPARISONS,
            "lexer_tokens_per_file_max": MAX_LEX_TOKENS_PER_FILE,
        },
        "seed_revision": subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=root, check=True,
            capture_output=True, text=True,
        ).stdout.strip(),
        "summary": baseline_summary,
    }


def _validate_workspace(root: Path) -> Path:
    """Refuse missing, unrelated, nested, or unmarked check roots."""
    try:
        resolved = root.resolve(strict=True)
        top = Path(subprocess.run(
            ["git", "rev-parse", "--show-toplevel"], cwd=resolved, check=True,
            capture_output=True, text=True,
        ).stdout.strip()).resolve(strict=True)
    except (OSError, subprocess.CalledProcessError) as error:
        raise RatchetError("root is not an existing Git workspace") from error
    if resolved != top or not resolved.is_dir():
        raise RatchetError("root must be the Git workspace root")
    tracked = set(item.decode("utf-8") for item in subprocess.run(
        ["git", "ls-files", "-z"], cwd=resolved, check=True, capture_output=True,
    ).stdout.split(b"\0") if item)
    if any(marker not in tracked for marker in WORKSPACE_MARKERS):
        raise RatchetError("Git workspace is missing OpenATCA identity markers")
    try:
        manifest = (resolved / "Cargo.toml").read_text(encoding="utf-8")
        ruleset = (resolved / "docs/RULESET.md").read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise RatchetError("OpenATCA workspace markers cannot be read") from error
    if "[workspace]" not in manifest or '"crates/oa-track"' not in manifest:
        raise RatchetError("Cargo manifest does not identify the OpenATCA workspace")
    if not ruleset.startswith("# THE RULESET — OpenATCA"):
        raise RatchetError("ruleset does not identify the OpenATCA workspace")
    return resolved


def _read_baseline(path: Path) -> dict[str, object]:
    before = path.stat(follow_symlinks=False)
    if not stat.S_ISREG(before.st_mode) or path.is_symlink():
        raise RatchetError("baseline must be a regular non-symlink file")
    if before.st_size < 0 or before.st_size > MAX_BASELINE_BYTES:
        raise RatchetError("baseline byte limit exceeded")
    with path.open("rb") as stream:
        payload = stream.read(MAX_BASELINE_BYTES + 1)
        after = os.fstat(stream.fileno())
    if (len(payload) != before.st_size or len(payload) > MAX_BASELINE_BYTES or
            (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) !=
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
        raise RatchetError("baseline changed while reading")
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RatchetError(f"baseline JSON is malformed: {error}") from error
    if not isinstance(value, dict):
        raise RatchetError("baseline root is not an object")
    return value


def compare(current: dict[str, object], baseline: dict[str, object]) -> dict[str, object]:
    if baseline.get("schema") != 1 or baseline.get("algorithm") != ALGORITHM or baseline.get("scope") != SCOPE:
        raise RatchetError("baseline schema, algorithm, or source scope does not match this checker")
    if baseline.get("workspace_binding") != WORKSPACE_BINDING:
        raise RatchetError("baseline is not bound to the OpenATCA workspace")
    revision = baseline.get("seed_revision")
    if not isinstance(revision, str) or len(revision) != 40 or any(ch not in "0123456789abcdef" for ch in revision):
        raise RatchetError("baseline seed revision is malformed")
    limits = baseline.get("limits")
    expected_limits = {
        "long_function_lines_exceeds": 150,
        "argument_count_exceeds": 7,
        "duplicate_window_tokens": WINDOW_TOKENS,
        "duplicate_minimum_distinct_identifiers": MIN_IDENTIFIERS,
        "tracked_source_files_max": MAX_SOURCE_FILES,
        "tracked_source_bytes_max": MAX_SOURCE_BYTES,
        "tracked_source_file_bytes_max": MAX_SOURCE_FILE_BYTES,
        "baseline_bytes_max": MAX_BASELINE_BYTES,
        "function_body_tokens_max": MAX_CODE_TOKENS,
        "duplicate_windows_max": MAX_DUPLICATE_WINDOWS,
        "duplicate_block_groups_max": MAX_DUPLICATE_BLOCK_GROUPS,
        "duplicate_comparisons_max": MAX_DUPLICATE_COMPARISONS,
        "lexer_tokens_per_file_max": MAX_LEX_TOKENS_PER_FILE,
    }
    if limits != expected_limits:
        raise RatchetError("baseline metric limits do not match the pinned checker")
    base_summary = baseline.get("summary")
    if not isinstance(base_summary, dict):
        raise RatchetError("baseline summary is malformed")
    metrics = {
        "long_function_count": (current["long_function_count"], base_summary.get("long_function_count")),
        "wide_argument_function_count": (current["wide_argument_function_count"], base_summary.get("wide_argument_function_count")),
        "duplicate_block_count": (current["duplicate_block_count"], base_summary.get("duplicate_block_count")),
    }
    errors = [f"{name} rose from {old} to {new}" for name, (new, old) in metrics.items()
              if not isinstance(old, int) or not isinstance(new, int) or new > old]
    base_frequencies = base_summary.get("duplicate_frequencies")
    now_frequencies = current.get("duplicate_frequencies")
    if not isinstance(base_frequencies, dict) or not isinstance(now_frequencies, dict):
        raise RatchetError("baseline or current duplicate inventory is malformed")
    current_details = current.get("duplicate_details")
    additions = []
    for fingerprint, count in sorted(now_frequencies.items()):
        old_count = base_frequencies.get(fingerprint, 0)
        if not isinstance(count, int) or not isinstance(old_count, int):
            raise RatchetError("duplicate frequency is malformed")
        if count > old_count:
            details = current_details.get(fingerprint, {}) if isinstance(current_details, dict) else {}
            additions.append({
                "fingerprint": fingerprint,
                "additional_duplicate_blocks": count - old_count,
                "token_window": details.get("token_window", []),
                "occurrences": details.get("occurrences", []),
            })
    return {
        "status": "FAIL" if errors else "PASS",
        "errors": errors,
        "current": {
            "tracked_source_files": current["tracked_source_files"],
            "function_count": current["function_count"],
            "long_function_count": current["long_function_count"],
            "wide_argument_function_count": current["wide_argument_function_count"],
            "duplicate_block_count": current["duplicate_block_count"],
        },
        "baseline": {
            "seed_revision": baseline.get("seed_revision"),
            **{key: value[1] for key, value in metrics.items()},
        },
        "new_or_increased_duplicates": additions,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--baseline", type=Path, default=BASELINE_PATH)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--check", action="store_true")
    action.add_argument("--write-baseline", action="store_true")
    args = parser.parse_args(argv)
    try:
        root = _validate_workspace(args.root)
        baseline_path = args.baseline if args.baseline.is_absolute() else root / args.baseline
        try:
            baseline_path.parent.resolve(strict=True).relative_to(root)
        except ValueError as error:
            raise RatchetError("baseline path must stay inside the OpenATCA workspace") from error
        except OSError as error:
            raise RatchetError("baseline parent directory must already exist") from error
        if args.write_baseline:
            baseline = build_baseline(root)
            target = baseline_path
            payload = (json.dumps(baseline, indent=2, sort_keys=True) + "\n").encode("utf-8")
            if len(payload) > MAX_BASELINE_BYTES:
                raise RatchetError("generated baseline byte limit exceeded")
            target.write_bytes(payload)
            print(json.dumps({"status": "BASELINE_WRITTEN", "path": str(args.baseline),
                              "summary": baseline["summary"]}, sort_keys=True))
            return 0
        baseline = _read_baseline(baseline_path)
        current = analyze_sources(_read_tracked_sources(root))
        report = compare(current, baseline)
        print(json.dumps(report, sort_keys=True))
        return 0 if report["status"] == "PASS" else 1
    except (RatchetError, OSError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(json.dumps({"status": "ERROR", "error": str(error)}, sort_keys=True), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
