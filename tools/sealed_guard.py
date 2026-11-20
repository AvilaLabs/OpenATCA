"""The Python side of the sealed hold-out guard (OA-SEAL-001 and following).

The sealed paths are listed once, in crates/oa-sealed/sealed_paths.txt; the Rust guard (crates/oa-sealed)
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

import functools
import os
import sys
from pathlib import Path

LIST_PATH = Path(__file__).resolve().parent.parent / "crates" / "oa-sealed" / "sealed_paths.txt"
MAX_LINKS = 40


class SealedError(Exception):
    """A path is sealed (or cannot be shown not to be)."""


def parse_list(text: str):
    """Returns (root, [(entry_text, parts, is_prefix)]). Anything not understood is an error."""
    # Only printable ASCII and newline; a line is `word`, one space, `value` (the Rust parser
    # reads the same rules, so the two cannot read the file differently).
    for ch in text:
        if ch != "\n" and not " " <= ch <= "~":
            raise ValueError(f"byte {ord(ch):#04x}: only printable ASCII and newline are allowed")
    root = None
    entries = []
    for i, line in enumerate(text.split("\n"), 1):
        if not line or line.startswith("#"):
            continue
        if " " not in line:
            raise ValueError(f"line {i}: expected `root <path>` or `seal <path>`")
        word, value = line.split(" ", 1)
        if not value or value.startswith(" ") or value.endswith(" "):
            raise ValueError(f"line {i}: extra white space around the value")
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


def _list_text() -> str:
    """The list file as ASCII, newlines untouched (no universal-newline translation)."""
    return LIST_PATH.read_bytes().decode("ascii")


def dump(text: str | None = None) -> str:
    """The list as the Rust guard's `List::dump` prints it."""
    root, entries = parse_list(_list_text() if text is None else text)
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
    if "\0" in path:
        raise SealedError("NUL in path")
    try:
        out = "/" if os.path.isabs(path) else os.getcwd()
    except OSError as e:
        raise SealedError(str(e)) from e
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
            r, self.entries = parse_list(_list_text() if text is None else text)
            self.broken = False
        except (ValueError, OSError, UnicodeError):
            # The list is unreadable: the guard fails closed and refuses every path.
            r, self.entries, self.broken = "", [], True
        if root is not None:
            r = os.fspath(root)
        try:
            self.root = _resolve(r) if r else ""
        except SealedError:
            self.root = r

    def check(self, path: str | os.PathLike) -> None:
        """Raises SealedError if `path` is, is inside, resolves into, or holds a sealed entry.

        The path is used exactly as given: a caller that expands `~` must do so first and then
        check and open the same expanded path."""
        given = os.fspath(path)
        if isinstance(given, bytes):
            given = os.fsdecode(given)
        if self.broken:
            raise SealedError(f"{given}: refused by OA-SEAL-001: the sealed list does not parse, so nothing is read")
        try:
            real = _resolve(given)
        except (SealedError, OSError, ValueError):
            raise SealedError(
                f"{given}: refused by OA-SEAL-001: the path cannot be resolved (a link loop, an unreadable link or a bad path)"
            ) from None
        self._check_resolved(given, real)

    def _check_resolved(self, given: str, real: str) -> None:
        given_names, real_names = _names(given), _names(real)
        inside = self.root and (real == self.root or real.startswith(self.root.rstrip("/") + "/"))
        below = _names(real[len(self.root):]) if inside else None
        for e in self.entries:
            if _named_in(e, given_names):
                how = "the path names"
            elif _named_in(e, real_names):
                how = "the path resolves to"
            elif below is not None and _matches_at(e, below):
                how = "the path is inside"
            elif below is not None and _is_ancestor(e, below):
                how = "the path is a directory that holds (name something below it)"
            else:
                continue
            raise SealedError(
                f"{given}: refused by OA-SEAL-001: {how} the sealed hold-out entry `{e[0]}` "
                "(crates/oa-sealed/sealed_paths.txt); sealed data is never read"
            )

    def open(self, path, mode="r", **kw):
        """Checks `path`, opens it for reading, then asks the kernel which file the descriptor is
        (/proc/self/fd/N) and refuses a sealed one, closing it: the opener every file a tool reads
        goes through. Read modes only. Without /proc only the check before the open is made."""
        if mode not in ("r", "rb"):
            raise ValueError("the guarded opener reads only")
        self.check(path)
        fd = os.open(path, os.O_RDONLY)
        try:
            try:
                real = os.readlink(f"/proc/self/fd/{fd}")
            except OSError:
                real = None
            if real is not None:
                self._check_resolved(os.fspath(path) if not isinstance(path, bytes) else os.fsdecode(path), real)
            return open(fd, mode, closefd=True, **kw)
        except BaseException:
            try:
                os.close(fd)
            except OSError:
                pass
            raise


@functools.lru_cache(maxsize=1)
def production() -> Guard:
    return Guard()


def open_read(path, mode="r", **kw):
    """Production opener (check, open, verify the opened file)."""
    return production().open(path, mode, **kw)


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
    if len(argv) == 2 and argv[0] == "--parse-list":
        # Parses a list file the way the guard does and prints it, or exits 2 (the parity test).
        try:
            print(end=dump(Path(argv[1]).read_bytes().decode("ascii")))
        except (ValueError, OSError):
            return 2
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
