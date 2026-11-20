"""The Python side of the sealed hold-out guard (OA-SEAL-001 and following).

The sealed paths are listed once, in tools/sealed_paths.txt; the Rust guard (tools/oa-sealed)
compiles in the same file, and a test checks that both parse it to the same set. There is no
switch of any kind (variable, flag or file) that turns this guard off.

A path is resolved before it is checked (relative paths, `..`, every symlink, a symlinked parent
directory; a path that does not exist yet through its nearest existing ancestor). A link that
cannot be resolved is refused. The check is on the resolved path under the data root and by name
on the path as given and as resolved. A directory that holds a sealed entry is refused too. Nothing
inside a sealed directory is listed or read to decide this.

Command line, for shell scripts: `sealed_guard.py PATH...` exits 0 if none is sealed and 2 with a
message naming the rule otherwise; `sealed_guard.py --dump` prints the parsed list.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

LIST_PATH = Path(__file__).resolve().parent / "sealed_paths.txt"
MAX_LINKS = 40


class SealedError(Exception):
    """A path is sealed (or cannot be shown not to be)."""


def parse_list(text: str):
    """Returns (root, [(entry_text, parts, is_prefix)]). Anything not understood is an error."""
    root = None
    entries = []
    for i, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if " " not in line:
            raise ValueError(f"line {i}: expected `root <path>` or `seal <path>`")
        word, value = line.split(" ", 1)
        value = value.strip()
        if word == "root":
            if root is not None or not value.startswith("/"):
                raise ValueError(f"line {i}: one absolute root only")
            root = value
        elif word == "seal":
            parts = value.split("/")
            if any(p in ("", ".", "..") for p in parts):
                raise ValueError(f"line {i}: bad sealed path")
            prefix = parts[-1].endswith("*")
            if prefix:
                parts[-1] = parts[-1][:-1]
                if not parts[-1]:
                    raise ValueError(f"line {i}: bare `*`")
            entries.append((value, parts, prefix))
        else:
            raise ValueError(f"line {i}: unknown word `{word}`")
    if root is None:
        raise ValueError("no root line")
    if not entries:
        raise ValueError("no seal lines")
    return root, entries


def dump(text: str | None = None) -> str:
    """The list as the Rust guard's `List::dump` prints it."""
    root, entries = parse_list(LIST_PATH.read_text(encoding="utf-8") if text is None else text)
    return f"root {root}\n" + "".join(f"seal {e[0]}\n" for e in entries)


def _names(path: str) -> list[str]:
    return [p for p in path.split("/") if p not in ("", ".", "..")]


def _matches_at(entry, names: list[str]) -> bool:
    _, parts, prefix = entry
    n = len(parts)
    if len(names) < n:
        return False
    for i in range(n):
        if i == n - 1 and prefix:
            if not names[i].startswith(parts[i]):
                return False
        elif names[i] != parts[i]:
            return False
    return True


def _named_in(entry, names: list[str]) -> bool:
    return any(_matches_at(entry, names[i:]) for i in range(len(names)))


def _is_ancestor(entry, names: list[str]) -> bool:
    parts = entry[1]
    return len(names) < len(parts) and parts[: len(names)] == names


def _resolve(path: str, depth: int = 0) -> str:
    if depth > MAX_LINKS:
        raise SealedError("link loop")
    out = "/" if os.path.isabs(path) else os.getcwd()
    for comp in path.split("/"):
        if comp in ("", "."):
            continue
        if comp == "..":
            out = os.path.dirname(out) or "/"
            continue
        cand = os.path.join(out, comp)
        try:
            out = os.path.realpath(cand, strict=True)
        except OSError:
            if os.path.islink(cand):
                try:
                    target = os.readlink(cand)
                except OSError as e:
                    raise SealedError(str(e)) from e
                out = _resolve(os.path.join(os.path.dirname(cand), target), depth + 1)
            else:
                out = cand
    return out


class Guard:
    """The guard for one data root; production code uses `production()`."""

    def __init__(self, text: str | None = None, root: str | os.PathLike | None = None):
        try:
            r, self.entries = parse_list(
                LIST_PATH.read_text(encoding="utf-8") if text is None else text
            )
            self.broken = False
        except (ValueError, OSError):
            # The list is unreadable: the guard fails closed and refuses every path.
            r, self.entries, self.broken = "", [], True
        if root is not None:
            r = os.fspath(root)
        self.root = _resolve(r) if r else ""

    def check(self, path: str | os.PathLike) -> None:
        """Raises SealedError if `path` is, is inside, resolves into, or holds a sealed entry."""
        given = os.fspath(path)
        if isinstance(given, bytes):
            given = os.fsdecode(given)
        given = os.path.expanduser(given)
        if self.broken:
            raise SealedError(f"{given}: refused by OA-SEAL-001: the sealed list does not parse, so nothing is read")
        try:
            real = _resolve(given)
        except SealedError:
            raise SealedError(
                f"{given}: refused by OA-SEAL-001: the path cannot be resolved (a link loop or an unreadable link)"
            ) from None

        def refuse(how: str, e):
            raise SealedError(
                f"{given}: refused by OA-SEAL-001: {how} the sealed hold-out entry `{e[0]}` "
                "(tools/sealed_paths.txt); sealed data is never read"
            )

        for e in self.entries:
            if _named_in(e, _names(given)):
                refuse("the path names", e)
            if _named_in(e, _names(real)):
                refuse("the path resolves to", e)
            if self.root and (real == self.root or real.startswith(self.root.rstrip("/") + "/")):
                below = _names(real[len(self.root):])
                if _matches_at(e, below):
                    refuse("the path is inside", e)
                if _is_ancestor(e, below):
                    refuse("the path is a directory that holds (name something below it)", e)

    def open(self, path, mode="r", **kw):
        """Checks `path`, then opens it: the opener every file a tool reads goes through."""
        self.check(path)
        return open(path, mode, **kw)


def production() -> Guard:
    return Guard()


def refuse(*paths) -> None:
    """Production check of every path that is not None; raises SealedError."""
    g = production()
    for p in paths:
        if p is not None:
            g.check(p)


def main(argv: list[str]) -> int:
    if argv == ["--dump"]:
        sys.stdout.write(dump())
        return 0
    if not argv or any(a.startswith("--") for a in argv):
        print("usage: sealed_guard.py PATH...   (no options)", file=sys.stderr)
        return 2
    try:
        refuse(*argv)
    except SealedError as e:
        print(f"sealed_guard: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
