#!/usr/bin/env python3
"""Fail when exported safety flag constants have no external source consumer.

This is a bounded source check, not a Rust type checker. It examines integer `pub const`
declarations with flag-style names or flag/bit documentation, then requires a non-test Rust
    source use in another workspace package that depends on the defining crate. This is a bounded
    lexical source check, not proof that the use is reachable in a built binary. Comments, string
    literals, declaration sites, and test-only items do not count as uses.
"""

from __future__ import annotations

import argparse
import os
import json
import re
import stat
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


SAFETY_PACKAGES = frozenset(
    {
        "oa-ca",
        "oa-msaw",
        "oa-safety",
        "oa-runway",
        "oa-display",
        "oa-migration-switch",
        "oa-track",
        "oa-track-core",
        "oa-feed",
        "oa-asterix",
        "oa-adapt",
    }
)
INTEGER_TYPES = frozenset(
    {"u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize"}
)
FLAG_PREFIXES = ("F_", "AF_", "O_", "T_", "M_", "FLAG_", "VIOLATION_", "HAS_")
FLAG_SUFFIXES = ("_FLAG", "_FLAGS", "_MASK", "_BIT")
FLAG_EXACT_NAMES = frozenset({"COMMON_LOCAL_FRAME"})
MAX_MANIFEST_BYTES = 1 << 20
MAX_SOURCE_FILE_BYTES = 4 << 20
MAX_ALLOWLIST_BYTES = 256 << 10
MAX_TOTAL_MANIFEST_BYTES = 16 << 20
MAX_TOTAL_SOURCE_BYTES = 256 << 20
MAX_MANIFEST_COUNT = 2048
MAX_PACKAGE_COUNT = 512
MAX_SOURCE_FILES_PER_PACKAGE = 8192
MAX_TOTAL_SOURCE_FILES = 65536
MAX_PACKAGE_SCAN_DIRECTORIES = 32768
TOKEN_RE = re.compile(
    r"[A-Za-z_][A-Za-z0-9_]*|(?:0[xX][0-9A-Fa-f_]+|0[bB][01_]+|0[oO][0-7_]+|[0-9][0-9_]*)"
    r"(?:[ui](?:8|16|32|64|128|size))?|::|<<|>>|&&|\|\||==|!=|<=|>=|=>|[^\s]"
)
RAW_STRING_RE = re.compile(r"(?:b)?r(#{0,255})\"")
INTEGER_RE = re.compile(r"^(0[xX][0-9A-Fa-f_]+|0[bB][01_]+|0[oO][0-7_]+|[0-9][0-9_]*)(?:[ui](?:8|16|32|64|128|size))?$")


@dataclass(frozen=True)
class Package:
    name: str
    crate_name: str
    root: Path
    dependencies: dict[str, str]


@dataclass(frozen=True)
class Flag:
    package: str
    crate_name: str
    name: str
    declaration: str


@dataclass(frozen=True)
class Finding:
    flag: Flag
    consumers: tuple[str, ...]
    ambiguous_name: bool = False

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.flag.package, self.flag.name, self.flag.declaration)


class BoundedReader:
    def __init__(self, workspace: Path, file_limit: int, total_limit: int):
        self.workspace = workspace
        self.file_limit = file_limit
        self.total_limit = total_limit
        self.bytes_read = 0
        self.cache: dict[Path, str] = {}

    def read_text(self, path: Path) -> str:
        key = path.absolute()
        if key in self.cache:
            return self.cache[key]
        try:
            relative = key.relative_to(self.workspace)
        except ValueError as exc:
            raise ValueError("source path is outside the configured workspace") from exc
        if any(part in {".", ".."} for part in relative.parts):
            raise ValueError("source path contains a workspace escape component")
        try:
            key.resolve(strict=True).relative_to(self.workspace)
        except (OSError, ValueError) as exc:
            raise ValueError("resolved source path is outside the configured workspace") from exc
        cursor = self.workspace
        for part in relative.parts:
            cursor = cursor / part
            try:
                info = cursor.lstat()
            except OSError as exc:
                raise ValueError("source path is missing or unreadable") from exc
            if stat.S_ISLNK(info.st_mode):
                raise ValueError("symlink source path refused")
        flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
        try:
            descriptor = os.open(key, flags)
        except OSError as exc:
            raise ValueError("source file could not be opened safely") from exc
        try:
            before = os.fstat(descriptor)
            if not stat.S_ISREG(before.st_mode):
                raise ValueError("source path is not a regular file")
            if before.st_size > self.file_limit:
                raise ValueError("source file exceeds its configured byte cap")
            if self.bytes_read + before.st_size > self.total_limit:
                raise ValueError("source scan exceeds its aggregate byte cap")
            chunks: list[bytes] = []
            remaining = self.file_limit + 1
            while remaining:
                chunk = os.read(descriptor, min(65536, remaining))
                if not chunk:
                    break
                chunks.append(chunk)
                remaining -= len(chunk)
            content = b"".join(chunks)
            after = os.fstat(descriptor)
            named = os.stat(key, follow_symlinks=False)
            if (before.st_dev, before.st_ino, before.st_size) != (after.st_dev, after.st_ino, after.st_size):
                raise ValueError("source file changed while being read")
            if (after.st_dev, after.st_ino) != (named.st_dev, named.st_ino):
                raise ValueError("source file identity changed while being read")
            if len(content) > self.file_limit or len(content) != after.st_size:
                raise ValueError("source file exceeded its stable byte bound")
            self.bytes_read += len(content)
            try:
                text = content.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise ValueError("source file is not valid UTF-8") from exc
            self.cache[key] = text
            return text
        finally:
            os.close(descriptor)


def _strip_rust_noncode(source: str) -> str:
    """Replace comments and literals with spaces, preserving code punctuation and newlines."""
    out = list(source)
    i = 0
    n = len(source)

    def blank(start: int, end: int) -> None:
        for pos in range(start, end):
            if out[pos] not in "\r\n":
                out[pos] = " "

    while i < n:
        if source.startswith("//", i):
            end = source.find("\n", i)
            if end < 0:
                end = n
            blank(i, end)
            i = end
            continue
        if source.startswith("/*", i):
            start = i
            i += 2
            depth = 1
            while i < n and depth:
                if source.startswith("/*", i):
                    depth += 1
                    i += 2
                elif source.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            blank(start, i)
            continue

        # Raw strings and byte raw strings: b?r###"..."###.
        raw = RAW_STRING_RE.match(source, i)
        if raw:
            start = i
            hashes = raw.group(1)
            i += raw.end()
            close = '"' + hashes
            end = source.find(close, i)
            i = n if end < 0 else end + len(close)
            blank(start, i)
            continue

        # Ordinary strings and byte strings.
        string_start = i
        if source.startswith('b"', i):
            i += 1
        if source[i] == '"':
            i += 1
            while i < n:
                if source[i] == "\\":
                    i += min(2, n - i)
                elif source[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            blank(string_start, i)
            continue

        # Char literals are removed, while Rust lifetimes remain code. Only treat an apostrophe
        # as a literal opener if a closing apostrophe occurs before whitespace/newline.
        if source[i] == "'":
            close = i + 1
            escaped = False
            while close < n and source[close] not in "\r\n \t":
                if source[close] == "'" and not escaped:
                    blank(i, close + 1)
                    i = close + 1
                    break
                if source[close] == "\\" and not escaped:
                    escaped = True
                else:
                    escaped = False
                close += 1
            else:
                i += 1
            continue

        i += 1
    return "".join(out)


def _remove_cfg_test_modules(source: str) -> str:
    clean = _strip_rust_noncode(source)
    attribute = re.compile(r"#\s*\[(?P<body>[^\]]*)\]")
    edits: list[tuple[int, int]] = []
    pos = 0
    while pos < len(clean):
        match = attribute.search(clean, pos)
        if match is None:
            break
        attr_start = match.start()
        cursor = match.end()
        attributes = [match.group("body").strip()]
        item_start = cursor
        while True:
            while item_start < len(clean) and clean[item_start].isspace():
                item_start += 1
            next_attr = attribute.match(clean, item_start)
            if next_attr is None:
                break
            attributes.append(next_attr.group("body").strip())
            item_start = next_attr.end()
        excluded = any(
            re.fullmatch(r"cfg\s*\(\s*test\s*\)", body)
            or re.fullmatch(r"cfg\s*\(\s*any\s*\(\s*\)\s*\)", body)
            or body == "test"
            for body in attributes
        )
        pos = item_start
        while pos < len(clean) and clean[pos].isspace():
            pos += 1
        # Skip an optional visibility and common function qualifiers before the item keyword.
        prefix = clean[pos:]
        visibility = re.match(r"pub(?:\s*\([^)]*\))?\s+", prefix)
        if visibility:
            pos += visibility.end()
        qualifiers = re.match(r"(?:(?:async|unsafe|default)\s+)+", clean[pos:])
        if qualifiers:
            pos += qualifiers.end()
        item = re.match(r"(mod|fn|impl|struct|enum|trait|use|const|static|type)\b", clean[pos:])
        if not item:
            pos = max(match.end(), pos)
            continue
        kind = item.group(1)
        item_start = pos
        pos += item.end()
        if kind in {"mod", "fn", "impl", "struct", "enum", "trait"}:
            parens = brackets = angles = 0
            body_start = None
            while pos < len(clean):
                char = clean[pos]
                if char == "(":
                    parens += 1
                elif char == ")":
                    parens = max(0, parens - 1)
                elif char == "[":
                    brackets += 1
                elif char == "]":
                    brackets = max(0, brackets - 1)
                elif char == "<":
                    angles += 1
                elif char == ">" and angles:
                    angles -= 1
                elif parens == brackets == angles == 0 and char == ";":
                    pos += 1
                    break
                elif parens == brackets == angles == 0 and char == "{":
                    body_start = pos
                    break
                pos += 1
            if body_start is not None:
                depth = 1
                pos = body_start + 1
                while pos < len(clean) and depth:
                    if clean[pos] == "{":
                        depth += 1
                    elif clean[pos] == "}":
                        depth -= 1
                    pos += 1
                if depth != 0:
                    continue
                if pos < len(clean) and clean[pos] == ";":
                    pos += 1
        else:
            depth = 0
            while pos < len(clean):
                if clean[pos] == "{":
                    depth += 1
                elif clean[pos] == "}":
                    depth = max(0, depth - 1)
                elif clean[pos] == ";" and depth == 0:
                    pos += 1
                    break
                pos += 1
        if excluded:
            edits.append((attr_start, pos))
    if not edits:
        return source
    chars = list(source)
    for start, end in edits:
        for pos in range(start, end):
            if chars[pos] not in "\r\n":
                chars[pos] = " "
    return "".join(chars)


def _tokens(source: str) -> list[str]:
    return TOKEN_RE.findall(_strip_rust_noncode(_remove_cfg_test_modules(source)))


def _preceding_doc(source: str, offset: int) -> str:
    lines = source[:offset].splitlines()
    docs: list[str] = []
    for line in reversed(lines):
        stripped = line.strip()
        if stripped.startswith("///") or stripped.startswith("//!"):
            docs.append(stripped[3:].strip())
        elif not stripped:
            if docs:
                break
        else:
            break
    return " ".join(reversed(docs)).lower()


class _Expr:
    """Small evaluator for integer bitmask constant expressions; no eval/exec is used."""

    def __init__(self, tokens: list[str], names: dict[str, int]):
        self.tokens = tokens
        self.names = names
        self.pos = 0

    def parse(self) -> int | None:
        try:
            value = self._or()
            if self.pos != len(self.tokens):
                return None
            return value
        except (ValueError, ZeroDivisionError):
            return None

    def _or(self) -> int:
        value = self._xor()
        while self._take("|"):
            value |= self._xor()
        return value

    def _xor(self) -> int:
        value = self._and()
        while self._take("^"):
            value ^= self._and()
        return value

    def _and(self) -> int:
        value = self._shift()
        while self._take("&"):
            value &= self._shift()
        return value

    def _shift(self) -> int:
        value = self._atom()
        while self.pos < len(self.tokens) and self.tokens[self.pos] in {"<<", ">>"}:
            op = self.tokens[self.pos]
            self.pos += 1
            shift = self._atom()
            if shift < 0 or shift > 127:
                raise ValueError
            value = value << shift if op == "<<" else value >> shift
        return value

    def _atom(self) -> int:
        if self._take("("):
            value = self._or()
            if not self._take(")"):
                raise ValueError
            return value
        if self._take("!"):
            # Width is not known from syntax alone; unary complement is not accepted as a flag
            # initializer because it is usually a sentinel/mask, not a single bit declaration.
            raise ValueError
        if self.pos >= len(self.tokens):
            raise ValueError
        token = self.tokens[self.pos]
        self.pos += 1
        if token in self.names:
            return self.names[token]
        match = INTEGER_RE.fullmatch(token)
        if not match:
            raise ValueError
        literal = match.group(1).replace("_", "")
        if literal.lower().startswith("0x"):
            return int(literal, 16)
        if literal.lower().startswith("0b"):
            return int(literal, 2)
        if literal.lower().startswith("0o"):
            return int(literal, 8)
        return int(literal, 10)

    def _take(self, token: str) -> bool:
        if self.pos < len(self.tokens) and self.tokens[self.pos] == token:
            self.pos += 1
            return True
        return False


def _flag_named(name: str, docs: str) -> bool:
    return (
        name.startswith(FLAG_PREFIXES)
        or name.endswith(FLAG_SUFFIXES)
        or name in FLAG_EXACT_NAMES
        or re.search(r"(?:\[[^]]+::flags\]|flag bit\b|bit \d+:|presence bit)", docs) is not None
    )


def _decls(package: Package, workspace: Path, files: list[Path], reader: BoundedReader) -> list[Flag]:
    found: list[Flag] = []
    for path in files:
        source = reader.read_text(path)
        named_flag_decl = re.search(
            r"\bpub\s+const\s+(?:F_|AF_|O_|T_|M_|FLAG_|VIOLATION_|HAS_)[A-Z0-9_]+\b|"
            r"\bpub\s+const\s+COMMON_LOCAL_FRAME\b|\bpub\s+const\s+[A-Z0-9_]+_(?:FLAG|FLAGS|MASK|BIT)\b",
            source,
        )
        documented_flag = re.search(r"\[(?:[^]]+::flags)\]|\bflag bit\b|\bbit \d+:|\bpresence bit\b", source)
        if named_flag_decl is None and documented_flag is None:
            continue
        clean = _strip_rust_noncode(source)
        toks = TOKEN_RE.findall(_strip_rust_noncode(_remove_cfg_test_modules(source)))
        # Locate public integer constants. Attribute visibility such as pub(crate) is not public
        # API, and is intentionally not considered.
        for i in range(len(toks) - 5):
            if toks[i : i + 2] != ["pub", "const"]:
                continue
            name = toks[i + 2]
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
                continue
            if toks[i + 3] != ":" or toks[i + 4] not in INTEGER_TYPES:
                continue
            eq = i + 5
            if eq >= len(toks) or toks[eq] != "=":
                continue
            end = eq + 1
            depth = 0
            while end < len(toks):
                if toks[end] in {"(", "[", "{"}:
                    depth += 1
                elif toks[end] in {")", "]", "}"}:
                    depth -= 1
                elif toks[end] == ";" and depth == 0:
                    break
                end += 1
            if end >= len(toks):
                continue
            # Require a one-hot value or a composed bit expression. This excludes integer enum
            # tags, layer numbers, and bit-width constants such as CAT021_POS_BITS.
            expr = toks[eq + 1 : end]
            value = _Expr(expr, {}).parse()
            composed_bit_expr = any(x in expr for x in ("|", "<<", "^"))
            named_mask = name.endswith(("_FLAGS", "_MASK"))
            if value is None and not (composed_bit_expr and _flag_named(name, "")):
                continue
            if value is not None and value <= 0:
                continue
            is_bit_value = composed_bit_expr or named_mask or (value is not None and value & (value - 1) == 0)
            if not is_bit_value:
                continue
            # Find the declaration offset in comment-preserving source. This keeps `/// flag`
            # semantics available without counting prose as a consumer.
            match = re.search(rf"\bpub\s+const\s+{re.escape(name)}\b", clean)
            if not match:
                continue
            docs = _preceding_doc(source, match.start())
            if not _flag_named(name, docs):
                continue
            found.append(
                Flag(
                    package.name,
                    package.crate_name,
                    name,
                    path.relative_to(workspace).as_posix(),
                )
            )
    return found


def _package_roots(workspace: Path, reader: BoundedReader) -> list[Package]:
    roots: list[Package] = []
    manifest_count = 0
    visited_directories = 0
    for base in (workspace / "crates", workspace / "tools"):
        if not base.is_dir():
            continue
        if base.is_symlink():
            raise ValueError("workspace package directory is a symlink")
        for current, directories, filenames in os.walk(base, followlinks=False):
            current_path = Path(current)
            visited_directories += 1
            if visited_directories > MAX_PACKAGE_SCAN_DIRECTORIES:
                raise ValueError("workspace package directory scan exceeds its configured cap")
            kept: list[str] = []
            for directory in directories:
                child = current_path / directory
                if directory in {"target", ".git", "tests", "examples"}:
                    continue
                if child.is_symlink():
                    raise ValueError("workspace package tree contains a symlink directory")
                kept.append(directory)
            directories[:] = sorted(kept)
            if "Cargo.toml" not in filenames:
                continue
            manifest = current_path / "Cargo.toml"
            manifest_count += 1
            if manifest_count > MAX_MANIFEST_COUNT:
                raise ValueError("workspace manifest count exceeds its configured cap")
            try:
                doc = tomllib.loads(reader.read_text(manifest))
            except tomllib.TOMLDecodeError as exc:
                raise ValueError("workspace contains a malformed Cargo manifest") from exc
            meta = doc.get("package")
            if not isinstance(meta, dict) or not isinstance(meta.get("name"), str):
                continue
            name = meta["name"]
            lib = doc.get("lib", {})
            crate_name = lib.get("name", name.replace("-", "_")) if isinstance(lib, dict) else name.replace("-", "_")
            deps: dict[str, str] = {}
            for section in ("dependencies", "build-dependencies"):
                values = doc.get(section, {})
                if isinstance(values, dict):
                    for key, value in values.items():
                        target_name = value.get("package", key) if isinstance(value, dict) else key
                        deps[key.replace("-", "_")] = target_name.replace("-", "_")
            target_sections = doc.get("target", {})
            if isinstance(target_sections, dict):
                for target in target_sections.values():
                    if isinstance(target, dict):
                        values = target.get("dependencies", {})
                        if isinstance(values, dict):
                            for key, value in values.items():
                                target_name = value.get("package", key) if isinstance(value, dict) else key
                                deps[key.replace("-", "_")] = target_name.replace("-", "_")
            roots.append(Package(name, crate_name, manifest.parent, deps))
            if len(roots) > MAX_PACKAGE_COUNT:
                raise ValueError("workspace package count exceeds its configured cap")
    return roots


def _source_files(package: Package) -> list[Path]:
    src = package.root / "src"
    if not src.is_dir():
        return []
    if src.is_symlink():
        raise ValueError("package source directory is a symlink")
    files: list[Path] = []
    directory_count = 0
    for current, directories, names in os.walk(src, followlinks=False):
        directory = Path(current)
        directory_count += 1
        if directory_count > MAX_SOURCE_FILES_PER_PACKAGE * 2:
            raise ValueError("package source directory count exceeds its configured cap")
        kept: list[str] = []
        for name in directories:
            child = directory / name
            if name in {"tests", "examples"}:
                continue
            if child.is_symlink():
                raise ValueError("package source tree contains a symlink directory")
            kept.append(name)
        directories[:] = kept
        for name in names:
            if name.endswith(".rs"):
                path = directory / name
                if path.is_symlink():
                    raise ValueError("package source tree contains a symlink file")
                files.append(path)
                if len(files) > MAX_SOURCE_FILES_PER_PACKAGE:
                    raise ValueError("package Rust source count exceeds its configured cap")
    return files


def _use_statements(tokens: list[str]) -> list[tuple[int, int, bool, list[str]]]:
    statements: list[tuple[int, int, bool, list[str]]] = []
    i = 0
    while i < len(tokens):
        start = i
        public = False
        if tokens[i] == "pub":
            public = True
            i += 1
            if i < len(tokens) and tokens[i] == "(":
                public = False
                depth = 1
                i += 1
                while i < len(tokens) and depth:
                    depth += tokens[i] == "("
                    depth -= tokens[i] == ")"
                    i += 1
        if i >= len(tokens) or tokens[i] != "use":
            i = start + 1
            continue
        end = i + 1
        depth = 0
        while end < len(tokens):
            if tokens[end] == "{":
                depth += 1
            elif tokens[end] == "}":
                depth -= 1
            elif tokens[end] == ";" and depth == 0:
                break
            end += 1
        if end < len(tokens):
            statements.append((start, end + 1, public, tokens[i + 1 : end]))
            i = end + 1
        else:
            i += 1
    return statements


def _extend_module_aliases(
    aliases: dict[str, str],
    statements: list[tuple[int, int, bool, list[str]]],
    exports: dict[str, dict[str, set[tuple[str, str, str]]]],
) -> None:
    for _, _, _, stmt in statements:
        if not stmt:
            continue
        target = aliases.get(stmt[0])
        if target is None:
            continue
        for index, token in enumerate(stmt):
            if token != "as" or index == 0 or index + 1 >= len(stmt):
                continue
            imported_name = stmt[index - 1]
            local_name = stmt[index + 1]
            # `use dep::FLAG as LOCAL` is an item alias; `use dep::module as LOCAL` and
            # `use dep as LOCAL` are namespace aliases usable in qualified paths.
            if imported_name not in exports.get(target, {}):
                aliases[local_name] = target


def _consumer_references(
    flags: list[Flag], packages: list[Package], files_by_package: dict[str, list[Path]], reader: BoundedReader
) -> dict[tuple[str, str, str], tuple[str, ...]]:
    flags_by_package: dict[str, dict[str, set[tuple[str, str, str]]]] = {}
    for flag in flags:
        flags_by_package.setdefault(flag.package, {}).setdefault(flag.name, set()).add(
            (flag.package, flag.name, flag.declaration)
        )
    crate_to_package = {package.crate_name: package.name for package in packages}
    eligible_names = set(flags_by_package)
    while True:
        previous = len(eligible_names)
        for package in packages:
            if any(crate_to_package.get(target) in eligible_names for target in package.dependencies.values()):
                eligible_names.add(package.name)
        if len(eligible_names) == previous:
            break
    relevant_packages = [package for package in packages if package.name in eligible_names]
    source_cache: dict[str, list[tuple[list[str], list[tuple[int, int, bool, list[str]]]]]] = {}
    names_pattern = re.compile(r"\b(?:" + "|".join(re.escape(flag.name) for flag in flags) + r")\b")
    for package in relevant_packages:
        source_cache[package.name] = []
        for path in files_by_package[package.name]:
            source = reader.read_text(path)
            if not names_pattern.search(source):
                continue
            tokens = _tokens(source)
            source_cache[package.name].append((tokens, _use_statements(tokens)))

    # Public re-exports forward the original flag identity. They do not count as consumers on
    # their own; a later production-source read of the exported constant is required.
    exports = {name: {**symbols} for name, symbols in flags_by_package.items()}
    changed = True
    while changed:
        changed = False
        for package in relevant_packages:
            alias_to_package: dict[str, str] = {}
            for alias, target_crate in package.dependencies.items():
                target = crate_to_package.get(target_crate)
                if target is not None:
                    alias_to_package[alias] = target
            for _, statements in source_cache[package.name]:
                _extend_module_aliases(alias_to_package, statements, exports)
                for _, _, is_public, stmt in statements:
                    if not is_public or not stmt:
                        continue
                    target = alias_to_package.get(stmt[0])
                    if target is None:
                        continue
                    source_exports = exports.get(target, {})
                    if "*" in stmt:
                        pairs = [(name, name, values) for name, values in source_exports.items()]
                    else:
                        pairs = []
                        for index, name in enumerate(stmt):
                            if name in source_exports:
                                exported_name = (
                                    stmt[index + 2]
                                    if stmt[index + 1 : index + 2] == ["as"] and index + 2 < len(stmt)
                                    else name
                                )
                                pairs.append((name, exported_name, source_exports[name]))
                    package_exports = exports.setdefault(package.name, {})
                    for _, exported_name, keys in pairs:
                        current = package_exports.setdefault(exported_name, set())
                        old_len = len(current)
                        current.update(keys)
                        changed |= len(current) != old_len

    consumers_by_key: dict[tuple[str, str, str], set[str]] = {
        (flag.package, flag.name, flag.declaration): set() for flag in flags
    }
    for package in relevant_packages:
        alias_to_package: dict[str, str] = {}
        for alias, target_crate in package.dependencies.items():
            target_package = crate_to_package.get(target_crate)
            if target_package is not None:
                alias_to_package[alias] = target_package
        # The use scan also needs public re-export aliases, which may not contain the original
        # constant name. Build a per-package prefilter from the symbols exposed by its direct
        # dependencies after the re-export closure above.
        for _, statements in source_cache[package.name]:
            _extend_module_aliases(alias_to_package, statements, exports)

        visible_names: set[str] = set()
        for target in alias_to_package.values():
            visible_names.update(exports.get(target, {}).keys())
        if not visible_names:
            continue
        visible_pattern = re.compile(r"\b(?:" + "|".join(re.escape(name) for name in sorted(visible_names)) + r")\b")
        use_sources = []
        for path in files_by_package[package.name]:
            source = reader.read_text(path)
            if not visible_pattern.search(source):
                continue
            tokens = _tokens(source)
            use_sources.append((tokens, _use_statements(tokens)))

        for tokens, statements in use_sources:
            imported: dict[str, set[tuple[str, str, str]]] = {}
            globs: dict[str, set[tuple[str, str, str]]] = {}
            excluded: set[int] = set()
            for start, end, _, stmt in statements:
                excluded.update(range(start, end))
                if not stmt:
                    continue
                target = alias_to_package.get(stmt[0])
                if target is None:
                    continue
                target_exports = exports.get(target, {})
                if "*" in stmt:
                    for name, keys in target_exports.items():
                        globs.setdefault(name, set()).update(keys)
                for index, name in enumerate(stmt):
                    if name in target_exports:
                        alias = stmt[index + 2] if stmt[index + 1 : index + 2] == ["as"] and index + 2 < len(stmt) else name
                        imported.setdefault(alias, set()).update(target_exports[name])

            for index, token in enumerate(tokens):
                if index in excluded:
                    continue
                for key in imported.get(token, set()) | globs.get(token, set()):
                    if package.name != key[0]:
                        consumers_by_key[key].add(package.name)
                if index >= 2 and tokens[index - 1] == "::":
                    j = index - 2
                    while j >= 2 and tokens[j - 1] == "::":
                        j -= 2
                    if j >= 0:
                        root_alias = tokens[j]
                        target = alias_to_package.get(root_alias)
                        if target is not None:
                            for key in exports.get(target, {}).get(token, set()):
                                if package.name != key[0]:
                                    consumers_by_key[key].add(package.name)
    return {key: tuple(sorted(consumers)) for key, consumers in consumers_by_key.items()}


def _load_allowlist(path: Path, workspace: Path) -> dict[tuple[str, str, str], str]:
    try:
        data = json.loads(BoundedReader(workspace, MAX_ALLOWLIST_BYTES, MAX_ALLOWLIST_BYTES).read_text(path))
    except (FileNotFoundError, OSError, json.JSONDecodeError) as exc:
        raise ValueError("safety flag allowlist is missing or invalid") from exc
    if not isinstance(data, dict) or data.get("version") != 1 or not isinstance(data.get("entries"), list):
        raise ValueError("allowlist must be version 1 with an entries array")
    entries: dict[tuple[str, str, str], str] = {}
    for row in data["entries"]:
        if not isinstance(row, dict):
            raise ValueError("allowlist entry must be an object")
        package, name, declaration, reason = (row.get(k) for k in ("package", "constant", "declaration", "reason"))
        if not all(isinstance(x, str) and x.strip() for x in (package, name, declaration, reason)):
            raise ValueError("allowlist entries require exact package, constant, declaration, and reason")
        key = (package, name, declaration)
        if key in entries:
            raise ValueError(f"duplicate allowlist entry: {package}:{name}:{declaration}")
        entries[key] = reason
    return entries


def check(workspace: Path, allowlist_path: Path) -> tuple[list[Finding], list[tuple[str, str, str]]]:
    try:
        workspace = workspace.resolve(strict=True)
    except OSError as exc:
        raise ValueError("configured workspace path is missing or invalid") from exc
    if workspace == Path("/"):
        raise ValueError("filesystem root cannot be the configured workspace")
    if not (workspace / "Cargo.toml").is_file():
        raise ValueError("workspace Cargo.toml is missing")
    manifests = BoundedReader(workspace, MAX_MANIFEST_BYTES, MAX_TOTAL_MANIFEST_BYTES)
    packages = _package_roots(workspace, manifests)
    package_by_name = {package.name: package for package in packages}
    if len(package_by_name) != len(packages):
        raise ValueError("workspace contains duplicate package metadata names")
    missing_safety_packages = sorted(SAFETY_PACKAGES - package_by_name.keys())
    if missing_safety_packages:
        raise ValueError("safety packages are missing: " + ", ".join(missing_safety_packages))
    files_by_package = {package.name: _source_files(package) for package in packages}
    total_source_files = sum(len(files) for files in files_by_package.values())
    if total_source_files > MAX_TOTAL_SOURCE_FILES:
        raise ValueError("workspace Rust source count exceeds its configured cap")
    source_reader = BoundedReader(workspace, MAX_SOURCE_FILE_BYTES, MAX_TOTAL_SOURCE_BYTES)
    flags: list[Flag] = []
    for name in SAFETY_PACKAGES:
        package = package_by_name.get(name)
        if package is not None:
            flags.extend(_decls(package, workspace, files_by_package[name], source_reader))
    allowlist = _load_allowlist(allowlist_path, workspace)
    consumers = _consumer_references(flags, packages, files_by_package, source_reader)
    name_counts: dict[tuple[str, str], int] = {}
    for flag in flags:
        identity = (flag.package, flag.name)
        name_counts[identity] = name_counts.get(identity, 0) + 1
    ambiguous_names = {identity for identity, count in name_counts.items() if count > 1}
    findings = [
        Finding(
            flag,
            () if (flag.package, flag.name) in ambiguous_names else consumers[(flag.package, flag.name, flag.declaration)],
            (flag.package, flag.name) in ambiguous_names,
        )
        for flag in flags
    ]
    violations = [
        finding
        for finding in findings
        if finding.ambiguous_name or (not finding.consumers and finding.key not in allowlist)
    ]
    stale = sorted(key for key in allowlist if key not in {finding.key for finding in findings})
    return violations, stale


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument(
        "--allowlist", type=Path, default=Path(__file__).resolve().parents[1] / "safety-flag-consumer-allowlist.json"
    )
    args = parser.parse_args(argv)
    try:
        violations, stale = check(args.root.resolve(), args.allowlist.resolve())
    except (OSError, ValueError) as exc:
        print(f"safety_flag_consumer_check_failed: {exc}", file=sys.stderr)
        return 2
    for finding in violations:
        f = finding.flag
        if finding.ambiguous_name:
            print(
                f"ambiguous_safety_flag_name package={f.package} constant={f.name} "
                f"declaration={f.declaration}; basename resolution refused"
            )
        print(f"unconsumed_safety_flag package={f.package} constant={f.name} declaration={f.declaration}")
    for package, name, declaration in stale:
        print(f"stale_safety_flag_allowlist package={package} constant={name} declaration={declaration}")
    if violations or stale:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
