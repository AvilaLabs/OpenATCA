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
import functools
import json
import os
import re
import stat
import sys
from pathlib import Path

LIST_PATH = Path(__file__).resolve().parent.parent / "crates" / "oa-sealed" / "sealed_paths.txt"
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
            if root is not None or not value.startswith("/"):
                raise ValueError(f"line {i}: one absolute root only")
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
        try:
            self.root = _resolve(r) if r else ""
        except SealedError:
            self.root = r

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
            st = os.fstat(fd)
            if stat.S_ISREG(st.st_mode) and st.st_nlink > 1:
                raise SealedError(
                    f"{path}: refused by OA-SEAL-002: the file has {st.st_nlink} hard links, so its other names could be sealed data under a neutral name; copy it instead"
                )
            if real is not None:
                self._check_resolved(os.fspath(path) if not isinstance(path, bytes) else os.fsdecode(path), real)
            return open(fd, mode, closefd=True, **kw)
        except BaseException:
            try:
                os.close(fd)
            except OSError:
                pass
            raise


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


def time_in_window(value, optional: bool = False, guard: "Guard | None" = None) -> bool:
    """Whether a time cell (nanoseconds since the Unix epoch, an int or strict integer text) is in a
    sealed window; a cell that is not a strict integer counts as in the window (fail closed). With
    `optional`, an empty cell is no time and is not in the window."""
    if isinstance(value, bool):
        return True
    if isinstance(value, int):
        return _in_window_ns(value, guard)
    if isinstance(value, str):
        if optional and value == "":
            return False
        if _STRICT_INT.fullmatch(value):
            return _in_window_ns(int(value), guard)
    return True


def row_in_window(row, columns, optional=(), guard: "Guard | None" = None) -> bool:
    """Whether a row (a mapping) must be dropped: any of `columns` is missing, unparsable or in a
    sealed window, or any non-empty `optional` column is."""
    for c in columns:
        if c not in row or time_in_window(row[c], guard=guard):
            return True
    return any(c in row and time_in_window(row[c], optional=True, guard=guard) for c in optional)


# ---- time tokens in text (recorded messages, frames, corpus files) ----------------------------

_NS = 1_000_000_000
_ZONE = r"(?:Z|z|[+-][0-9]{2}(?::?[0-9]{2})?)"
_EXT = re.compile(
    r"(?<![0-9])([0-9]{4})-([0-9]{2})-([0-9]{2})"
    r"(?:[Tt ]([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:[.,]([0-9]+))?)?(" + _ZONE + r")?)?(?![0-9])", re.ASCII)
_BASIC = re.compile(
    r"(?<![0-9])([0-9]{4})([0-9]{2})([0-9]{2})[Tt]([0-9]{2})(?:([0-9]{2})(?:([0-9]{2})(?:[.,]([0-9]+))?)?)?("
    + _ZONE + r")?(?![0-9])", re.ASCII)
_RUN = re.compile(r"(?<![0-9])[0-9]+(?![0-9])", re.ASCII)
_ENTITY = re.compile(r"&(#[0-9]+|#[xX][0-9a-fA-F]+|[A-Za-z_][A-Za-z0-9_.:-]*);", re.ASCII)
_BAD_AMP = re.compile(r"&#")
_NAMED = {"amp": "&", "lt": "<", "gt": ">", "quot": '"', "apos": "'"}
# A time written without a zone could be any local time: the instant is checked over every zone
# from UTC-12 to UTC+14.
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


def _token_hit(y, mo, d, h, mi, s, frac, zone, guard) -> bool:
    if h is None:
        lo = _civil_ns(y, mo, d)
        hi = lo + 24 * 3600 * _NS
        zone_ns = None
    else:
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
        return _overlaps_window(lo - _NAIVE_AFTER_NS, hi + _NAIVE_BEFORE_NS, guard)
    return _overlaps_window(lo - zone_ns, hi - zone_ns, guard)


def _scan_text(text: str, guard) -> bool:
    for m in _EXT.finditer(text):
        y, mo, d, h, mi, s, frac, zone = m.groups()
        try:
            if _token_hit(int(y), int(mo), int(d), h, mi, s, frac, zone, guard):
                return True
        except _Unparsable:
            return True
    for m in _BASIC.finditer(text):
        y, mo, d, h, mi, s, frac, zone = m.groups()
        try:
            if _token_hit(int(y), int(mo), int(d), h, mi, s, frac, zone, guard):
                return True
        except _Unparsable:
            return True
    for m in _RUN.finditer(text):
        run = m.group(0)
        n = len(run)
        try:
            if n in (8, 10, 12, 14) and 1970 <= int(run[:4]) <= 2200:
                # compact civil date or date-time (YYYYMMDD[HH[MM[SS]]]); only a date that does not
                # parse fails closed, for the other lengths an invalid one is just a number
                try:
                    parts = [int(run[:4]), int(run[4:6]), int(run[6:8])]
                    parts += [run[8:10] or None, run[10:12] or None, run[12:14] or None]
                    if _token_hit(*parts, None, None, guard):
                        return True
                except _Unparsable:
                    if n == 8:
                        return True
            scale = {10: _NS, 13: 1_000_000, 16: 1_000, 19: 1}.get(n)
            if scale is not None:
                ns = int(run) * scale
                if _overlaps_window(ns, ns + scale, guard):
                    return True
        except ValueError:
            return True
    return False


def text_has_sealed_time(data: bytes, guard: "Guard | None" = None) -> bool:
    """Conservative: whether `data` (a recorded message or frame payload) holds a time in a sealed
    window. XML numeric references are decoded first. Every ISO 8601 date-time token (extended and
    basic forms, with or without Z, a fraction or an offset), every date-only token (the whole day),
    and every 8 to 19 digit run that reads as a compact date, a compact date-time or a Unix time in
    s, ms, us or ns is checked. True also when a token looks like a date but does not parse, when an
    entity is left that is not decoded, and when the sealed list does not parse."""
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
        if _scan_text(decoded, g):
            return True
    return False


# ---- OAF1 frames and ADS-B traces --------------------------------------------------------------

OAF1_HEADER = 36
OAF1_TRAILER = 4


def oaf1_frames(data: bytes):
    """The OAF1 frames of a stream as (frame bytes, header recv_unix_ns, jms_timestamp_ms, payload),
    layout as in docs/FEED_FRAMES.md (magic 0..4, kind, flags, reserved, seq 8..16, recv_unix_ns
    16..24 i64, jms_timestamp_ms 24..32 i64, payload_len 32..36 u32, payload, crc32), and the number
    of bytes after the last whole frame."""
    import struct
    out = []
    i = 0
    while i + OAF1_HEADER + OAF1_TRAILER <= len(data) and data[i:i + 4] == b"OAF1":
        n = struct.unpack("<I", data[i + 32:i + 36])[0]
        end = i + OAF1_HEADER + n + OAF1_TRAILER
        if end > len(data):
            break
        recv, jms = struct.unpack("<qq", data[i + 16:i + 32])
        out.append((data[i:end], recv, jms, data[i + OAF1_HEADER:i + OAF1_HEADER + n]))
        i = end
    return out, len(data) - i


def oaf1_frame_in_window(recv_ns: int, jms_ms: int, payload: bytes, guard: "Guard | None" = None) -> bool:
    """Whether a frame is dropped: its receive time (or its JMS time, when set) is in the window, or
    its payload holds a sealed time by `text_has_sealed_time`."""
    if _in_window_ns(recv_ns, guard):
        return True
    if jms_ms != 0 and _in_window_ns(jms_ms * 1_000_000, guard):
        return True
    return text_has_sealed_time(payload, guard)


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
    frames (each frame's header time and payload; a stream that is cut or has bytes after the last
    frame is scanned whole too) and the trace shape."""
    if text_has_sealed_time(data, guard):
        return True
    if data[:4] == b"OAF1":
        frames, rest = oaf1_frames(data)
        for _, recv, jms, payload in frames:
            if oaf1_frame_in_window(recv, jms, payload, guard):
                return True
        if rest:
            return True
    return adsb_trace_in_window(data, guard)


def filter_corpus_dir(directory, counter: DropCounter, guard: "Guard | None" = None):
    """Deletes every file below `directory` that `file_has_sealed_time` flags (files named `syn-*` or
    `seed-*` are the committed synthetic seeds and stay); a file that cannot be read by the guarded
    opener is deleted too. Returns (scanned, removed, kept); nothing about a file is printed."""
    g = guard or production()
    g.check(directory, True)
    scanned = removed = 0
    for base, dirs, files in os.walk(os.fspath(directory), followlinks=False):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            scanned += 1
            if name.startswith(("syn-", "seed-")):
                continue
            try:
                with g.open(path, "rb") as f:
                    bad = file_has_sealed_time(f.read(), g)
            except (SealedError, OSError, MemoryError):
                bad = True
            if bad:
                os.unlink(path)
                removed += 1
                counter.add()
    return scanned, removed, scanned - removed


def walk_unsealed(root):
    """Production walk: (unsealed files, sealed entries skipped, directories already visited)."""
    return production().walk_unsealed(root)


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
