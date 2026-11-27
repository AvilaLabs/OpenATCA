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

import atexit
import calendar
import errno
import fnmatch
import functools
import hashlib
import json
import os
import re
import stat
import struct
import sys
import zlib
from pathlib import Path
from typing import NamedTuple

LIST_PATH = Path(__file__).resolve().parent.parent / "crates" / "oa-sealed" / "sealed_paths.txt"
DEFAULT_DATA_ROOT = LIST_PATH.parent / "../../openatca-data"
MAX_LINKS = 40
MAX_WALK_DEPTH = 64


_DAY = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", re.ASCII)
_UTC = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})Z")


class SealedError(Exception):
    """A path is sealed (or cannot be shown not to be)."""


def _parse_utc_ns(t: str):
    """`YYYY-MM-DDTHH:MM:SSZ` (exactly that shape) as nanoseconds since the Unix epoch, or None."""
    m = _UTC.fullmatch(t)
    if not m:
        return None
    y, mo, d, h, mi, sec = (int(g) for g in m.groups())
    if not 1970 <= y <= 2200 or not 1 <= mo <= 12 or d < 1 or h > 23 or mi > 59 or sec > 59:
        return None
    if d > calendar.monthrange(y, mo)[1]:
        return None
    return calendar.timegm((y, mo, d, h, mi, sec)) * 1_000_000_000


# How far each listed window is widened on both sides: 15 minutes, in nanoseconds (OA-SEAL-011).
WINDOW_MARGIN_NS = 15 * 60 * 1_000_000_000


def parse_list(text: str):
    """Returns (root, [(entry_text, parts, kind)], days, [(window_text, from_ns, to_ns)]); kind is
    "exact", "prefix" (`X*`) or "contains" (`*X*`); days is (days, scopes, exempt), the days as
    written and the scope and exempt directories as (text, parts). Anything not understood is an
    error."""
    # Only printable ASCII and newline; a line is `word`, one space, `value` (the Rust parser
    # reads the same rules, so the two cannot read the file differently).
    for ch in text:
        if ch != "\n" and not " " <= ch <= "~":
            raise ValueError(f"byte {ord(ch):#04x}: only printable ASCII and newline are allowed")
    root = None
    entries = []
    windows = []
    days: list[str] = []
    scopes: list = []
    exempt: list = []
    for i, line in enumerate(text.split("\n"), 1):
        if not line or line.startswith("#"):
            continue
        if " " not in line:
            raise ValueError(f"line {i}: expected `root <path>`, `seal <path>` or `window <from> <to>`")
        word, value = line.split(" ", 1)
        if not value or value.startswith(" ") or value.endswith(" "):
            raise ValueError(f"line {i}: extra white space around the value")
        if word == "root":
            if root is not None or not (value.startswith("/") or value == "$OPENATCA_DATA"):
                raise ValueError(f"line {i}: one absolute root or `$OPENATCA_DATA` marker only")
            root = value
        elif word == "seal":
            parts = value.split("/")
            if any(p in ("", ".", "..") for p in parts):
                raise ValueError(f"line {i}: bad sealed path")
            kind = "exact"
            if parts[-1].endswith("*"):
                parts[-1] = parts[-1][:-1]
                kind = "prefix"
                if parts[-1].startswith("*"):
                    parts[-1] = parts[-1][1:]
                    kind = "contains"
                if not parts[-1] or "*" in parts[-1]:
                    raise ValueError(f"line {i}: bare or inner `*`")
            elif "*" in parts[-1]:
                raise ValueError(f"line {i}: `*` only as `X*` or `*X*`")
            if any("*" in p for p in parts[:-1]):
                raise ValueError(f"line {i}: `*` only in the last component")
            entries.append((value, parts, kind))
        elif word == "day":
            if not _DAY.fullmatch(value) or not 1 <= int(value[5:7]) <= 12 or not 1 <= int(value[8:10]) <= 31:
                raise ValueError(f"line {i}: expected `day YYYY-MM-DD`")
            days.append(value)
        elif word in ("dayscope", "dayexempt"):
            parts = value.split("/")
            if any(p in ("", ".", "..") or "*" in p for p in parts):
                raise ValueError(f"line {i}: bad day scope path")
            (scopes if word == "dayscope" else exempt).append((value, parts))
        elif word == "window":
            bits = value.split(" ", 1)
            ns = [_parse_utc_ns(b) for b in bits]
            if len(bits) != 2 or None in ns:
                raise ValueError(f"line {i}: expected `window <from-UTC> <to-UTC>`")
            if ns[0] >= ns[1]:
                raise ValueError(f"line {i}: the window is empty or reversed")
            # Widened by the margin on each side (the listed text is kept), as in `oa-sealed`.
            windows.append((value, ns[0] - WINDOW_MARGIN_NS, ns[1] + WINDOW_MARGIN_NS))
        else:
            raise ValueError(f"line {i}: unknown word `{word}`")
    if root is None:
        raise ValueError("no root line")
    if not entries:
        raise ValueError("no seal lines")
    if bool(days) != bool(scopes):
        raise ValueError("`day` and `dayscope` lines come together")
    return root, entries, (days, scopes, exempt), windows


def _list_text() -> str:
    """The list file as ASCII, newlines untouched (no universal-newline translation)."""
    return LIST_PATH.read_bytes().decode("ascii")


def dump(text: str | None = None) -> str:
    """The list as the Rust guard's `List::dump` prints it."""
    root, entries, (days, scopes, exempt), windows = parse_list(_list_text() if text is None else text)
    return (f"root {root}\n" + "".join(f"seal {e[0]}\n" for e in entries)
            + "".join(f"day {d}\n" for d in days)
            + "".join(f"dayscope {t[0]}\n" for t in scopes)
            + "".join(f"dayexempt {t[0]}\n" for t in exempt)
            + "".join(f"window {w[0]}\n" for w in windows))


def _names(path: str) -> list[str]:
    return [p for p in path.split("/") if p not in ("", ".", "..")]


def _matches_at(entry, names: list[str]) -> bool:
    _, parts, kind = entry
    n = len(parts)
    if len(names) < n:
        return False
    if names[: n - 1] != parts[: n - 1]:
        return False
    last = parts[-1]
    if kind == "exact":
        return names[n - 1] == last
    if kind == "prefix":
        return names[n - 1].startswith(last)
    return any(last in a for a in names[n - 1:])


def _named_in(entry, names: list[str]) -> bool:
    return any(_matches_at(entry, names[i:]) for i in range(len(names)))


def _is_ancestor(entry, names: list[str]) -> bool:
    parts = entry[1]
    # A substring entry seals names, not a folder: its directory part is an ordinary folder.
    return entry[2] != "contains" and len(names) < len(parts) and parts[: len(names)] == names


def _compact(name: str) -> str:
    return name.replace(".", "").replace("-", "").replace("_", "")


def _day_in(days: list[str], rest: list[str]):
    """The sealed day a component of `rest` (below a scope) names, else None."""
    norm = [_compact(c) for c in rest]
    for d in days:
        c = _compact(d)
        y, m, dd, ym = c[:4], c[4:6], c[6:], c[:6]
        if (any(c in n for n in norm)
                or any(norm[k:k + 3] == [y, m, dd] for k in range(len(norm)))
                or any(norm[k:k + 2] == [ym, dd] for k in range(len(norm)))):
            return d
    return None


def _day_hit(rules, names: list[str], anchored: bool):
    """(scope, day) when `names` has a scope as consecutive components (at the start only when
    `anchored`) and a component below it names a sealed day, unless it is under an exempt path."""
    days, scopes, exempt = rules
    for i in ([0] if anchored else range(len(names))):
        t = names[i:]
        for text, parts in scopes:
            if t[: len(parts)] != parts or len(t) <= len(parts):
                continue
            if any(t[: len(e[1])] == e[1] for e in exempt):
                continue
            d = _day_in(days, t[len(parts):])
            if d is not None:
                return text, d
    return None


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
            r, self.entries, self.days, self.windows = parse_list(_list_text() if text is None else text)
            self.broken = False
        except (ValueError, OSError, UnicodeError):
            # The list is unreadable: the guard fails closed and refuses every path.
            r, self.entries, self.days, self.windows, self.broken = "", [], ([], [], []), [], True
        if root is not None:
            r = os.fspath(root)
        elif r == "$OPENATCA_DATA":
            configured = os.environ.get("OPENATCA_DATA")
            if configured == "":
                self.broken = True
                self.root = ""
                return
            r = configured if configured is not None else os.fspath(DEFAULT_DATA_ROOT)
            if not os.path.isabs(r):
                self.broken = True
                self.root = ""
                return
        try:
            if not r:
                self.root = ""
                self.broken = True
                return
            if not os.path.isabs(r):
                raise SealedError("data root is not absolute")
            resolved = _resolve(r)
            existing = Path(r)
            while True:
                try:
                    if existing.is_symlink() and not existing.exists():
                        raise SealedError("data root contains an unresolved symlink")
                    if not stat.S_ISDIR(existing.stat().st_mode):
                        raise SealedError("data root ancestor is not a directory")
                    break
                except FileNotFoundError:
                    parent = existing.parent
                    if parent == existing:
                        raise SealedError("data root has no existing ancestor")
                    existing = parent
            self.root = resolved
        except (SealedError, OSError, ValueError):
            self.root = ""
            self.broken = True

    def in_window(self, unix_ns: int) -> bool:
        """Whether `unix_ns` (nanoseconds since the Unix epoch, UTC) is in a sealed window
        (half-open). A guard whose list does not parse seals every instant."""
        return self.broken or not self.windows or any(w[1] <= unix_ns < w[2] for w in self.windows)

    def walk_unsealed(self, root: str | os.PathLike):
        """(files, skipped, already_visited): the unsealed files below `root`, sorted; a directory
        or file the guard refuses is skipped and counted, never descended into (a directory that
        only holds a sealed entry, such as the data root or raw/adsblol, is walked); a directory
        already visited (a link back, or a second name of one directory; entries are walked in
        bytewise order, so the first name is the one walked) is counted apart. `root` itself refused raises SealedError; a missing
        root raises OSError, and nesting deeper than MAX_WALK_DEPTH raises SealedError. The files
        are sorted bytewise by the string form of the path."""
        self.check(root, True)
        os.stat(root)
        files: list[str] = []
        skipped = 0
        again = 0
        visited: set[str] = set()

        def walk(d: str, depth: int) -> None:
            nonlocal skipped, again
            if not os.path.isdir(d):
                files.append(d)
                return
            if depth > MAX_WALK_DEPTH:
                raise SealedError(
                    f"{d}: directories nested deeper than {MAX_WALK_DEPTH}; the walk stops with an error, not silently"
                )
            real = os.path.realpath(d)
            if real in visited:
                again += 1
                return
            visited.add(real)
            for name in sorted(os.listdir(d), key=os.fsencode):
                p = os.path.join(d, name)
                try:
                    self.check(p, True)
                except SealedError:
                    skipped += 1
                    continue
                walk(p, depth + 1)

        walk(os.fspath(root), 0)
        return sorted(files, key=os.fsencode), skipped, again

    def check(self, path: str | os.PathLike, walking: bool = False) -> None:
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
        self._check_resolved(given, real, walking)

    def _check_resolved(self, given: str, real: str, walking: bool = False) -> None:
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
            elif not walking and below is not None and _is_ancestor(e, below):
                how = "the path is a directory that holds (name something below it)"
            else:
                continue
            raise SealedError(
                f"{given}: refused by OA-SEAL-001: {how} the sealed hold-out entry `{e[0]}` "
                "(crates/oa-sealed/sealed_paths.txt); sealed data is never read"
            )
        for how, names, anchored in (("the path names", given_names, False),
                                     ("the path resolves to", real_names, False),
                                     ("the path is inside", below, True)):
            hit = None if names is None else _day_hit(self.days, names, anchored)
            if hit:
                raise SealedError(
                    f"{given}: refused by OA-SEAL-001: {how} the sealed hold-out entry `day {hit[1]} below {hit[0]}` "
                    "(crates/oa-sealed/sealed_paths.txt); sealed data is never read"
                )

    def verify_fd(self, fd: int, path) -> None:
        """Asks the kernel which file the descriptor is (/proc/self/fd/N) and refuses a sealed one, and a
        regular file with more than one hard link; SealedError. Without /proc only the link count is checked."""
        try:
            real = os.readlink(f"/proc/self/fd/{fd}")
        except OSError:
            real = None
        st = os.fstat(fd)
        if stat.S_ISREG(st.st_mode) and st.st_nlink > 1:
            raise SealedError(
                f"{path}: refused by OA-SEAL-002: the file has {st.st_nlink} hard links, so its other names could be sealed data under a neutral name; copy it instead"
            )
        if real is not None:
            self._check_resolved(os.fspath(path) if not isinstance(path, bytes) else os.fsdecode(path), real)

    def open(self, path, mode="r", **kw):
        """Checks `path`, opens it for reading, then asks the kernel which file the descriptor is
        (/proc/self/fd/N) and refuses a sealed one, closing it: the opener every file a tool reads
        goes through. Read modes only. Without /proc only the check before the open is made."""
        if mode not in ("r", "rb"):
            raise ValueError("the guarded opener reads only")
        self.check(path)
        fd = os.open(path, os.O_RDONLY)
        try:
            self.verify_fd(fd, path)
            return open(fd, mode, closefd=True, **kw)
        except BaseException:
            try:
                os.close(fd)
            except OSError:
                pass
            raise

    def open_regular_read(self, path, mode="rb", **kw):
        """Check, open nonblocking, verify the opened descriptor, then require a regular file.

        This narrow reader is for bounded offline tools that must refuse FIFOs and devices without
        waiting on them. Generic ``open`` keeps its existing semantics. No bytes are read until
        both the pathname and the opened descriptor have passed the guard.
        """
        if mode not in ("r", "rb"):
            raise ValueError("the guarded regular-file opener reads only")
        if not hasattr(os, "O_NONBLOCK"):
            raise OSError(errno.ENOTSUP, "nonblocking regular-file open is unavailable")
        self.check(path)
        flags = os.O_RDONLY | os.O_NONBLOCK
        flags |= getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOCTTY", 0)
        fd = os.open(path, flags)
        try:
            self.verify_fd(fd, path)
            st = os.fstat(fd)
            if not stat.S_ISREG(st.st_mode):
                raise OSError(errno.EINVAL, f"{path}: guarded input must be a regular file")
            result = open(fd, mode, closefd=True, **kw)
            fd = -1
            return result
        finally:
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError:
                    pass


def start_warning(g: Guard) -> str | None:
    """The warning a tool prints at start when the list seals every instant: a list that does not
    parse (which also refuses every sealed path) or has no window line; None otherwise."""
    if g.broken:
        return "seal list does not parse: every timed row is dropped and every sealed path refused"
    if not g.windows:
        return "seal list has no window: every timed row is dropped"
    return None


@functools.lru_cache(maxsize=1)
def production() -> Guard:
    g = Guard()
    # Say so once (the guard is cached), on stderr, at start.
    w = start_warning(g)
    if w is not None:
        print(w, file=sys.stderr)
    return g


def open_read(path, mode="r", **kw):
    """Production opener (check, open, verify the opened file)."""
    return production().open(path, mode, **kw)


def in_sealed_window(unix_ns: int) -> bool:
    """Production: whether `unix_ns` is in a sealed window of the list."""
    return production().in_window(unix_ns)


def _in_window_ns(unix_ns: int, guard: "Guard | None") -> bool:
    return (guard or production()).in_window(unix_ns)


class DropCounter:
    """Counts the rows a Python tool drops because they fall in a sealed window and prints
    `sealed-window rows dropped: N` (also when N is 0) once, at exit, also after an error, as the Rust
    tools do. The line goes to standard error so a tool's own report on standard output keeps its shape."""

    def __init__(self, announce: bool = True):
        self.count = 0
        self._printed = False
        if announce:
            atexit.register(self.report)

    def add(self, n: int = 1) -> None:
        self.count += n

    def line(self) -> str:
        return f"sealed-window rows dropped: {self.count}"

    def report(self) -> None:
        if not self._printed:
            self._printed = True
            print(self.line(), file=sys.stderr)


_STRICT_INT = re.compile(r"-?[0-9]+", re.ASCII)


def time_in_window(value, optional: bool = False, guard: "Guard | None" = None, keep_malformed: bool = False,
                   loose: bool = False) -> bool:
    """Whether a time cell (nanoseconds since the Unix epoch, an int or integer text) is in a sealed
    window. A cell that does not parse counts as in the window (fail closed; text is then read as
    Python's `int()` reads it), unless `keep_malformed`: then it is not in the window and the caller's
    own validation rejects it, as before. With `keep_malformed` text must be `-?[0-9]+` to parse, or
    anything `int()` reads when `loose` (for a reader that itself reads text with `int()`). With
    `optional`, an empty cell is no time and is not in the window."""
    if isinstance(value, bool):
        return not keep_malformed
    if isinstance(value, int):
        return _in_window_ns(value, guard)
    if isinstance(value, str):
        if optional and value == "":
            return False
        try:
            if keep_malformed and not loose:
                return _in_window_ns(int(value), guard) if _STRICT_INT.fullmatch(value) else False
            return _in_window_ns(int(value), guard)
        except ValueError:
            pass   # also a cell over Python's integer-text limit (4300 digits): not a time
    return not keep_malformed


def row_in_window(row, columns, optional=(), guard: "Guard | None" = None, keep_malformed: bool = False,
                  loose: bool = False) -> bool:
    """Whether a row (a mapping) must be dropped: any of `columns` is in a sealed window, or any
    non-empty `optional` column is. A missing or unparsable time also drops the row, unless
    `keep_malformed` (scoring readers: the row goes on to their own validation, which rejects it);
    `loose` as in `time_in_window`."""
    for c in columns:
        if c not in row:
            if keep_malformed:
                continue
            return True
        if time_in_window(row[c], guard=guard, keep_malformed=keep_malformed, loose=loose):
            return True
    return any(c in row and time_in_window(row[c], optional=True, guard=guard, keep_malformed=keep_malformed, loose=loose)
               for c in optional)


# ---- time tokens in text (recorded messages, frames, corpus files) ----------------------------

_NS = 1_000_000_000
_DAY_NS = 86_400 * _NS
_ZONE = r"(?:Z|z|[+-][0-9]{2}(?::?[0-9]{2})?)"
# No look-around on digits anywhere: digits glued to a date must not hide it.
_EXT = re.compile(
    r"([0-9]{4})([-/.])([0-9]{1,2})([-/.])([0-9]{1,2})"
    r"(?:[Tt ]([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:[.,]([0-9]+))?)?(" + _ZONE + r")?)?", re.ASCII)
_BASIC = re.compile(
    r"([0-9]{4})([0-9]{2})([0-9]{2})[Tt]([0-9]{2})(?:([0-9]{2})(?:([0-9]{2})(?:[.,]([0-9]+))?)?)?("
    + _ZONE + r")?", re.ASCII)
_DMY_NUM = re.compile(r"([0-9]{1,2})[-/.]([0-9]{1,2})[-/.]([0-9]{4})", re.ASCII)
_YY_NUM = re.compile(r"([0-9]{2})[-/.]([0-9]{2})[-/.]([0-9]{2})", re.ASCII)
_MON = "jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec"
_MONTH_NO = {m: i for i, m in enumerate(_MON.split("|"), 1)}
_NSEP = r"[\s.,/-]*"
_DMY_NAME = re.compile(r"([0-9]{1,2})(?:st|nd|rd|th)?" + _NSEP + "(" + _MON + r")[a-z]*\.?" + _NSEP + r"([0-9]{4}|[0-9]{2})",
                       re.ASCII | re.IGNORECASE)
_MDY_NAME = re.compile("(" + _MON + r")[a-z]*\.?" + _NSEP + r"([0-9]{1,2})(?:st|nd|rd|th)?" + _NSEP + r"([0-9]{4})",
                       re.ASCII | re.IGNORECASE)
_MY_NAME = re.compile("(" + _MON + r")[a-z]*" + _NSEP + r"([0-9]{4})", re.ASCII | re.IGNORECASE)
_ORDINAL = re.compile(r"([0-9]{4})-([0-9]{3})", re.ASCII)
_WEEK = re.compile(r"([0-9]{4})-?[Ww]([0-9]{2})(?:-?([0-9]))?", re.ASCII)
_RUN = re.compile(r"[0-9]+", re.ASCII)
_CHAIN = re.compile(r"[0-9]+(?:[,_ \xa0  ][0-9]+)+")
_SCI = re.compile(r"(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)[eE][+-]?[0-9]+", re.ASCII)
_ENTITY = re.compile(r"&(#[0-9]+|#[xX][0-9a-fA-F]+|[A-Za-z_][A-Za-z0-9_.:-]*);", re.ASCII)
_BAD_AMP = re.compile(r"&#")
_NAMED = {"amp": "&", "lt": "<", "gt": ">", "quot": '"', "apos": "'"}
# Epoch values are judged in s, ds, cs, ms, us, 100 ns and ns.
_EPOCH_SCALES = (10 ** 9, 10 ** 8, 10 ** 7, 10 ** 6, 10 ** 3, 100, 1)
# A time written without a zone could be any local time, UTC-12 to UTC+14: local time L is UTC
# L-14h to L+12h.
_NAIVE_BEFORE_NS = 14 * 3600 * _NS
_NAIVE_AFTER_NS = 12 * 3600 * _NS


class _Unparsable(Exception):
    pass


def _decode_entities(text: str):
    """Numeric character references and the five predefined entities decoded (repeatedly, so a
    reference that spells another one is also decoded); None if an entity is left that is not one of
    those (unknown name, bad or out-of-range number, a `&#` that is not a reference)."""
    for _ in range(8):
        bad = False

        def sub(m):
            nonlocal bad
            body = m.group(1)
            if body[0] == "#":
                try:
                    n = int(body[2:], 16) if body[1] in "xX" else int(body[1:])
                    if n == 0 or n > 0x10FFFF or 0xD800 <= n <= 0xDFFF:
                        raise ValueError
                    return chr(n)
                except ValueError:
                    bad = True
                    return m.group(0)
            if body in _NAMED:
                return _NAMED[body]
            bad = True
            return m.group(0)

        new = _ENTITY.sub(sub, text)
        if bad:
            return None
        if new == text:
            break
        text = new
    else:
        return None
    # what is still `&#...` is a broken reference; any other `&name;` was caught above
    return None if _BAD_AMP.search(text) else text


def _overlaps_window(lo: int, hi: int, guard: "Guard | None") -> bool:
    """Whether [lo, hi) (ns) meets a sealed window (half-open, margin included)."""
    g = guard or production()
    if g.broken or not g.windows:
        return True
    return any(lo < w[2] and hi > w[1] for w in g.windows)


def _civil_ns(y, mo, d, h=0, mi=0, s=0):
    if not 1900 <= y <= 2300 or not 1 <= mo <= 12 or d < 1 or d > calendar.monthrange(y, mo)[1]:
        raise _Unparsable
    if h > 24 or mi > 59 or s > 60 or (h == 24 and (mi or s)):
        raise _Unparsable
    return calendar.timegm((y, mo, d, 0, 0, 0)) * _NS + (h * 3600 + mi * 60 + s) * _NS


def _zone_ns(z):
    """The zone's offset from UTC in ns, or None when there is no zone."""
    if z is None:
        return None
    if z in ("Z", "z"):
        return 0
    sign = 1 if z[0] == "+" else -1
    hh = int(z[1:3])
    mm = int(z[4:6] if ":" in z else z[3:5] or 0)
    if hh > 23 or mm > 59:
        raise _Unparsable
    return sign * (hh * 3600 + mm * 60) * _NS


def _days_hit(y, mo, d, n, guard) -> bool:
    """`n` whole days from the date (no zone: any zone)."""
    lo = _civil_ns(y, mo, d)
    return _overlaps_window(lo - _NAIVE_BEFORE_NS, lo + n * _DAY_NS + _NAIVE_AFTER_NS, guard)


def _token_hit(y, mo, d, h, mi, s, frac, zone, guard) -> bool:
    if h is None:
        return _days_hit(y, mo, d, 1, guard)
    lo = _civil_ns(y, mo, d, int(h), int(mi or 0), int(s or 0))
    if frac is not None:
        width = _NS // (10 ** min(len(frac), 9)) or 1
        lo += int(frac[:9].ljust(9, "0"))
    elif s is not None:
        width = _NS
    elif mi is not None:
        width = 60 * _NS
    else:
        width = 3600 * _NS
    hi = lo + width
    zone_ns = _zone_ns(zone)
    if zone_ns is None:
        return _overlaps_window(lo - _NAIVE_BEFORE_NS, hi + _NAIVE_AFTER_NS, guard)
    return _overlaps_window(lo - zone_ns, hi - zone_ns, guard)


def _loose_date_hit(y, mo, d, n, guard) -> bool:
    """A date in a loose form (its reading is a guess): one that does not parse is ignored."""
    try:
        return _days_hit(y, mo, d, n, guard)
    except _Unparsable:
        return False


def _epoch_hit(lo_v, hi_v, guard) -> bool:
    return any(_overlaps_window(int(lo_v * sc), int(hi_v * sc) + 1, guard) for sc in _EPOCH_SCALES)


MAX_RUN_DIGITS = 4096


def _scan_run(run: str, guard, strict8: bool = True) -> bool:
    """A run of digits as a compact date or date-time, a day of the year, or an epoch time. A run
    over 19 digits is read at every position (every substring of 7 to 19 digits); one over
    MAX_RUN_DIGITS is judged sealed (fail closed). `strict8`: an 8-digit date that does not parse
    counts as sealed."""
    import datetime
    n = len(run)
    if n > 19:
        if n > MAX_RUN_DIGITS:
            return True
        return any(_scan_run(run[i:i + k], guard, False) for k in range(7, 20) for i in range(n - k + 1))
    if n >= 4 and 1970 <= int(run[:4]) <= 2200:
        if n == 7:
            ddd = int(run[4:])
            if 1 <= ddd <= 366:
                try:
                    d = datetime.date(int(run[:4]), 1, 1) + datetime.timedelta(days=ddd - 1)
                    if d.year == int(run[:4]) and _days_hit(d.year, d.month, d.day, 1, guard):
                        return True
                except (ValueError, _Unparsable):
                    pass
        if n in (8, 10, 12, 14, 15, 16, 17):
            # YYYYMMDD[HH[MM[SS[fraction]]]]; only a date that does not parse fails closed
            try:
                if _token_hit(int(run[:4]), int(run[4:6]), int(run[6:8]), run[8:10] or None, run[10:12] or None,
                              run[12:14] or None, run[14:] or None, None, guard):
                    return True
            except _Unparsable:
                if n == 8 and strict8:
                    return True
    if 9 <= n <= 19:
        v = int(run)
        if _epoch_hit(v, v + 1, guard):
            return True
    return False


def _scan_sci(token: str, guard) -> bool:
    import decimal
    try:
        v = decimal.Decimal(token)
        mant, _, exp = token.lower().partition("e")
        decimals = len(mant.partition(".")[2])
        e = int(exp) - decimals
        if abs(e) > 40 or not v.is_finite() or v > 10 ** 30:
            return False
        unit = decimal.Decimal(1).scaleb(e)
        return _epoch_hit(int(v - unit), int(v + unit) + 1, guard)
    except (decimal.InvalidOperation, ValueError):
        return False


def _scan_text(text: str, guard) -> bool:
    import datetime
    spans = []   # date-times judged to the instant: the loose readers below do not read them again
    for m in _EXT.finditer(text):
        y, s1, mo, s2, d, h, mi, s, frac, zone = m.groups()
        try:
            if _token_hit(int(y), int(mo), int(d), h, mi, s, frac, zone, guard):
                return True
            if h is not None:
                spans.append(m.span())
        except _Unparsable:
            if s1 == "-" and s2 == "-":
                return True
    for m in _BASIC.finditer(text):
        y, mo, d, h, mi, s, frac, zone = m.groups()
        try:
            if _token_hit(int(y), int(mo), int(d), h, mi, s, frac, zone, guard):
                return True
            spans.append(m.span())
        except _Unparsable:
            return True
    if spans:
        pieces, at = [], 0
        for a, b in sorted(spans):
            if a >= at:
                pieces += [text[at:a], " "]
                at = b
        text = "".join(pieces) + text[at:]
    for m in _DMY_NUM.finditer(text):
        a, b, y = (int(g) for g in m.groups())
        if _loose_date_hit(y, b, a, 1, guard) or _loose_date_hit(y, a, b, 1, guard):
            return True
    for m in _YY_NUM.finditer(text):
        a, b, c = (int(g) for g in m.groups())
        if (_loose_date_hit(2000 + a, b, c, 1, guard) or _loose_date_hit(2000 + c, b, a, 1, guard)
                or _loose_date_hit(2000 + c, a, b, 1, guard)):
            return True
    for m in _DMY_NAME.finditer(text):
        d, mon, y = m.groups()
        y = int(y) if len(y) == 4 else 2000 + int(y)
        if _loose_date_hit(y, _MONTH_NO[mon.lower()], int(d), 1, guard):
            return True
    for m in _MDY_NAME.finditer(text):
        mon, d, y = m.groups()
        if _loose_date_hit(int(y), _MONTH_NO[mon.lower()], int(d), 1, guard):
            return True
    for m in _MY_NAME.finditer(text):
        mon, y = m.groups()
        mo, y = _MONTH_NO[mon.lower()], int(y)
        if 1900 <= y <= 2300 and _loose_date_hit(y, mo, 1, calendar.monthrange(y, mo)[1], guard):
            return True
    for m in _ORDINAL.finditer(text):
        y, ddd = int(m.group(1)), int(m.group(2))
        if 1900 <= y <= 2300 and 1 <= ddd <= 366:
            try:
                dt = datetime.date(y, 1, 1) + datetime.timedelta(days=ddd - 1)
            except (ValueError, OverflowError):
                continue
            if dt.year == y and _loose_date_hit(dt.year, dt.month, dt.day, 1, guard):
                return True
    for m in _WEEK.finditer(text):
        y, w, dd = int(m.group(1)), int(m.group(2)), m.group(3)
        try:
            first = datetime.date.fromisocalendar(y, w, int(dd) if dd else 1)
            n = 1 if dd else 7
            if _days_hit(first.year, first.month, first.day, n, guard):
                return True
        except (ValueError, _Unparsable):
            return True
    for m in _RUN.finditer(text):
        if _scan_run(m.group(0), guard):
            return True
    for m in _CHAIN.finditer(text):
        groups = re.split(r"[^0-9]", m.group(0))
        for i in range(len(groups)):
            acc = ""
            for g in groups[i:]:
                acc += g
                if len(acc) > 19:
                    break
                if _scan_run(acc, guard):
                    return True
    for m in _SCI.finditer(text):
        if _scan_sci(m.group(0), guard):
            return True
    return False


_MARKUP = re.compile(r"<!--.*?-->|<!\[CDATA\[|\]\]>|<[^>]*>|\s+", re.DOTALL)


def _strip_markup(text: str) -> str:
    """The text with comments, CDATA marks, tags and every white space removed: a time split by
    markup or a line break reads whole."""
    return _MARKUP.sub("", text)


def text_has_sealed_time(data: bytes, guard: "Guard | None" = None) -> bool:
    """Conservative: whether `data` (a recorded message or frame payload) holds a time in a sealed
    window. XML numeric references are decoded first, and the text is read both as it is and with
    comments, tags and white space removed. Every ISO 8601 date-time token (extended and basic
    forms, with or without Z, a fraction or an offset), every date-only token (the whole day), the
    day/month/year forms with `-`, `/` or `.`, two-digit years, month names, ordinal and week
    dates, compact dates and date-times, and every run of 9 to 19 digits (also with `,`, `_` or a
    space between groups, and in scientific form) read as an epoch time in s, ds, cs, ms, us, 100 ns
    or ns are checked. True also when a token looks like a date but does not parse, when an entity
    is left that is not decoded, and when the sealed list does not parse."""
    g = guard or production()
    if g.broken or not g.windows:
        return True
    if isinstance(data, str):
        data = data.encode("utf-8", "surrogatepass")
    # latin-1 maps every byte to one character, so nothing is lost for a binary payload; a copy
    # with the NULs removed reads UTF-16 text
    views = [data.decode("latin-1")]
    if b"\0" in data:
        views.append(data.replace(b"\0", b"").decode("latin-1"))
    if data[:3] == b"\xef\xbb\xbf":
        views.append(data[3:].decode("utf-8", "replace"))
    try:
        views.append(data.decode("utf-8"))
    except UnicodeDecodeError:
        pass
    for v in views:
        decoded = _decode_entities(v)
        if decoded is None:
            return True
        if _scan_text(decoded, g) or _scan_text(_strip_markup(decoded), g):
            return True
    return False


# ---- recording directories ----------------------------------------------------------------------

# at every position, overlapping, no look-around: digits glued to a date do not hide it
_PATH_DATE = re.compile(r"(?=([0-9]{4})([-._]?)([0-9]{2})\2([0-9]{2}))")


def path_date(relpath: str):
    """The latest date written anywhere in a path as YYYYMMDD, YYYY-MM-DD, YYYY.MM.DD or YYYY_MM_DD
    (found at every position, whatever digits touch it), as a (year, month, day) tuple; None when the
    path has no date or has one in years 1970 to 2200 that is not a real date."""
    import datetime
    found = []
    for m in _PATH_DATE.finditer(relpath):
        y, _, mo, d = m.groups()
        if not 1970 <= int(y) <= 2200:
            continue
        try:
            found.append(datetime.date(int(y), int(mo), int(d)))
        except ValueError:
            return None
    if not found:
        return None
    last = max(found)
    return last.year, last.month, last.day


# ---- OAF frames and ADS-B traces ----------------------------------------------------------------

OAF_MAGIC = {b"OAF1": 36, b"OAF2": 44}   # header bytes before the payload (docs/FEED_FRAMES.md)
OAF_TRAILER = 4
OAF_MAX_PAYLOAD = 4 * 1024 * 1024
OAF_KINDS = (1, 2, 3)


class Frame(NamedTuple):
    raw: bytes
    kind: int
    recv_ns: int
    jms_ms: int
    payload: bytes
    crc_ok: bool


def oaf_frames(data: bytes):
    """The OAF1 and OAF2 frames of a stream and the number of bytes after the last whole frame.
    Layout as in docs/FEED_FRAMES.md and crates/oa-feed/src/core/frame.rs, little-endian: magic 0..4,
    kind 4, flags 5, reserved 6..8, seq 8..16, recv_unix_ns 16..24 (i64), jms_timestamp_ms 24..32
    (i64); an OAF1 frame has payload_len (u32) at 32 and the payload at 36; an OAF2 frame has mono_ns
    (u64) at 32..40, payload_len at 40 and the payload at 44; then the payload and a CRC-32 (IEEE) of
    every preceding byte of the frame. A frame is cut by its length field; one whose CRC is wrong is
    still returned, flagged."""
    out = []
    i = 0
    while i + 4 <= len(data) and data[i:i + 4] in OAF_MAGIC:
        head = OAF_MAGIC[data[i:i + 4]]
        if i + head + OAF_TRAILER > len(data):
            break
        n = struct.unpack("<I", data[i + head - 4:i + head])[0]
        end = i + head + n + OAF_TRAILER
        if n > OAF_MAX_PAYLOAD or end > len(data):
            break
        recv, jms = struct.unpack("<qq", data[i + 16:i + 32])
        crc_ok = struct.unpack("<I", data[end - 4:end])[0] == zlib.crc32(data[i:end - 4])
        out.append(Frame(data[i:end], data[i + 4], recv, jms, data[i + head:i + head + n], crc_ok))
        i = end
    return out, len(data) - i


def oaf_frame_dropped(f: Frame, guard: "Guard | None" = None) -> bool:
    """Whether a frame is dropped: a bad header (unknown kind, receive time not above 0, a negative
    JMS time, a CRC that does not match), its receive time or its JMS time (when set) in a sealed
    window, or a sealed time in its payload by `text_has_sealed_time`."""
    if f.kind not in OAF_KINDS or f.recv_ns <= 0 or f.jms_ms < 0 or not f.crc_ok:
        return True
    if _in_window_ns(f.recv_ns, guard):
        return True
    if f.jms_ms != 0 and _in_window_ns(f.jms_ms * 1_000_000, guard):
        return True
    return text_has_sealed_time(f.payload, guard)


def adsb_trace_in_window(raw: bytes, guard: "Guard | None" = None) -> bool:
    """Whether an ADS-B trace file (readsb JSON: `timestamp`, then `trace` rows whose first number is
    the seconds after it) is dropped: any row time is in the window, or the text scan finds one. A
    file that is not that shape is judged by the text scan alone; a trace row without a number time
    is dropped."""
    if text_has_sealed_time(raw, guard):
        return True
    try:
        doc = json.loads(raw)
    except (ValueError, RecursionError):
        return False
    if not isinstance(doc, dict) or "trace" not in doc:
        return False
    ts = doc.get("timestamp")
    trace = doc["trace"]
    if isinstance(ts, bool) or not isinstance(ts, (int, float)) or not isinstance(trace, list):
        return True
    for row in trace:
        if not isinstance(row, list) or not row or isinstance(row[0], bool) or not isinstance(row[0], (int, float)):
            return True
        t = ts + row[0]
        if t != t or abs(t) > 1e12:
            return True
        ns = int(t * _NS)
        if _overlaps_window(ns - _NS, ns + _NS, guard):
            return True
    return False


def file_has_sealed_time(data: bytes, guard: "Guard | None" = None) -> bool:
    """The test `make_corpora.py --filter-existing` applies to a corpus file: the text scan, the OAF1
    and OAF2 frames (each by `oaf_frame_dropped`; a stream that is cut or has bytes after the last
    frame is scanned whole too) and the trace shape."""
    if text_has_sealed_time(data, guard):
        return True
    if data[:4] in OAF_MAGIC:
        frames, rest = oaf_frames(data)
        if rest or any(oaf_frame_dropped(f, guard) for f in frames):
            return True
    return adsb_trace_in_window(data, guard)


def corpus_dir_check(directory, guard: "Guard | None" = None) -> str:
    """The resolved path of `directory` if it is a corpus directory, `<data root>/fuzz/<target>/corpus`
    or a directory below it, and passes the guard's check; SealedError otherwise (the data root, raw,
    scds, derived, fuzz itself and every sealed directory are refused); NotADirectoryError if it is
    not a directory."""
    g = guard or production()
    g.check(directory)
    real = _resolve(os.fspath(directory))
    root = g.root.rstrip("/")
    names = _names(real[len(root):]) if root and real.startswith(root + "/") else []
    if len(names) < 3 or names[0] != "fuzz" or names[2] != "corpus":
        raise SealedError(
            f"{os.fspath(directory)}: refused: only a corpus directory (<data root>/fuzz/<target>/corpus, "
            "or a directory below it) is filtered")
    if not os.path.isdir(real):
        raise NotADirectoryError(f"{os.fspath(directory)}: not a directory")
    return real


def filter_corpus_dir(directory, counter: DropCounter, guard: "Guard | None" = None, exempt_sha256=frozenset()):
    """Deletes every regular file below a corpus directory that `file_has_sealed_time` flags,
    whatever its name; a file whose bytes (sha256) are one of `exempt_sha256` (the committed seeds)
    stays. Entries the guard refuses by name are skipped (not listed, not counted). The walk is by
    file descriptor: the top directory is opened once (O_NOFOLLOW) and checked to still be the
    resolved corpus path, every directory and file is opened relative to its parent's descriptor
    with O_NOFOLLOW, a directory on another device is not entered, and no path string is resolved
    again, so a swap of a name for a link cannot redirect a read or a delete. A file is removed
    only if the descriptor it was read through still is the file at that name (device and inode)
    and has one hard link. A file the guard refuses on opening, or that cannot be read or removed,
    is never deleted: it is counted as refused. Anything that is not a regular file (FIFO, device,
    socket, link) is skipped and counted. Returns (scanned, removed, kept, refused, skipped);
    nothing about a file is printed."""
    g = guard or production()
    top = corpus_dir_check(directory, g)
    dflags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    ffl = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    n = {"scanned": 0, "removed": 0, "refused": 0, "skipped": 0}

    def walk(dfd: int, dpath: str, dev: int, depth: int) -> None:
        for name in sorted(os.listdir(dfd), key=os.fsencode):
            path = os.path.join(dpath, name)
            try:
                st = os.stat(name, dir_fd=dfd, follow_symlinks=False)
            except OSError:
                n["scanned"] += 1
                n["refused"] += 1
                continue
            try:
                g.check(path, stat.S_ISDIR(st.st_mode))
            except SealedError:
                continue
            if stat.S_ISDIR(st.st_mode):
                if st.st_dev != dev or depth >= MAX_WALK_DEPTH:
                    n["scanned"] += 1
                    n["skipped"] += 1
                    continue
                try:
                    cfd = os.open(name, dflags, dir_fd=dfd)
                except OSError:
                    n["scanned"] += 1
                    n["refused"] += 1
                    continue
                try:
                    now = os.fstat(cfd)
                    if (now.st_dev, now.st_ino) != (st.st_dev, st.st_ino):
                        n["scanned"] += 1
                        n["refused"] += 1
                        continue
                    walk(cfd, path, dev, depth + 1)
                finally:
                    os.close(cfd)
                continue
            n["scanned"] += 1
            if not stat.S_ISREG(st.st_mode):
                n["skipped"] += 1
                continue
            try:
                fd = os.open(name, ffl, dir_fd=dfd)
            except OSError:
                n["refused"] += 1
                continue
            try:
                fst = os.fstat(fd)
                if not stat.S_ISREG(fst.st_mode):
                    n["skipped"] += 1
                    continue
                g.verify_fd(fd, path)
                with open(fd, "rb", closefd=False) as f:
                    data = f.read()
                if hashlib.sha256(data).hexdigest() in exempt_sha256 or not file_has_sealed_time(data, g):
                    continue
                again = os.stat(name, dir_fd=dfd, follow_symlinks=False)
                if (again.st_dev, again.st_ino) != (fst.st_dev, fst.st_ino) or os.fstat(fd).st_nlink != 1:
                    n["refused"] += 1
                    continue
                os.unlink(name, dir_fd=dfd)
                n["removed"] += 1
                counter.add()
            except (SealedError, OSError, MemoryError):
                n["refused"] += 1
            finally:
                os.close(fd)

    tfd = os.open(top, dflags)
    try:
        try:
            same = os.readlink(f"/proc/self/fd/{tfd}") == top
        except OSError:
            same = False
        if not same:
            raise SealedError(f"{os.fspath(directory)}: refused: the corpus directory changed while it was opened")
        walk(tfd, top, os.fstat(tfd).st_dev, 0)
    finally:
        os.close(tfd)
    return n["scanned"], n["removed"], n["scanned"] - n["removed"], n["refused"], n["skipped"]


def walk_unsealed(root):
    """Production walk: (unsealed files, sealed entries skipped, directories already visited)."""
    return production().walk_unsealed(root)


def getsize(path) -> int:
    """`os.path.getsize` after the guard's check."""
    production().check(path)
    return os.path.getsize(path)


def exists(path) -> bool:
    """`os.path.exists` after the guard's check (a sealed path raises SealedError)."""
    production().check(path)
    return os.path.exists(path)


def isdir(path) -> bool:
    """`os.path.isdir` after the guard's check."""
    production().check(path)
    return os.path.isdir(path)


def listdir(path) -> list[str]:
    """The names in a directory, bytewise sorted; the directory must pass the guard's check (a
    directory that only holds a sealed entry is listed, its sealed entries are left out)."""
    g = production()
    g.check(path, True)
    out = []
    for name in sorted(os.listdir(path), key=os.fsencode):
        try:
            g.check(os.path.join(os.fspath(path), name), True)
        except SealedError:
            continue
        out.append(name)
    return out


def glob(pattern: str) -> list[str]:
    """`glob.glob` for `*`, `?` and `[...]` in any component, listing only directories the guard
    allows and returning only paths it allows (a sealed one is never listed or returned). Hidden
    names match only a pattern that starts with a dot, as in glob."""
    magic = re.compile(r"[*?\[]")
    parts = pattern.split("/")
    results = ["/" if pattern.startswith("/") else ""]
    for i, comp in enumerate(parts):
        if comp == "" and i == 0:
            continue
        nxt = []
        for base in results:
            if not magic.search(comp):
                cand = os.path.join(base, comp) if base else comp
                try:
                    production().check(cand, True)
                except SealedError:
                    continue
                if os.path.lexists(cand):
                    nxt.append(cand)
                continue
            try:
                names = listdir(base or ".")
            except (SealedError, OSError):
                continue
            for name in names:
                if name.startswith(".") and not comp.startswith("."):
                    continue
                if fnmatch.fnmatchcase(name, comp):
                    nxt.append(os.path.join(base, name) if base else name)
        results = nxt
    final = []
    for p in sorted(results):
        try:
            production().check(p)
        except SealedError:
            continue
        final.append(p)
    return final


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
