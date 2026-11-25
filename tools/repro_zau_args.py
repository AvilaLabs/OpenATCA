"""Small validators for reproduction-manifest clip options and ADS-B release part names."""
from __future__ import annotations

import re
import sys
from datetime import date


_TAG = re.compile(r"v(?P<date>[0-9]{4}\.[0-9]{2}\.[0-9]{2})-planes-readsb-prod-(?P<prod>[0-9]{1,3})")
_PART = re.compile(r"(?P<tag>v[0-9]{4}\.[0-9]{2}\.[0-9]{2}-planes-readsb-prod-[0-9]{1,3})\.tar\.(?P<part>[a-z]{2})")
_RELEASE_ROOT = "https://github.com/adsblol/globe_history_2026/releases"


def _tag_day(tag: str) -> str:
    match = _TAG.fullmatch(tag)
    if match is None or len(tag) > 64:
        raise ValueError(f"invalid adsb.lol release tag {tag!r}")
    raw_day = match.group("date")
    if not raw_day.startswith("2026."):
        raise ValueError("this reproduction package is pinned to the 2026 adsb.lol release namespace")
    try:
        parsed = date.fromisoformat(raw_day.replace(".", "-"))
    except ValueError as exc:
        raise ValueError(f"invalid calendar date in adsb.lol release tag {tag!r}") from exc
    return parsed.isoformat()


def release_tag(name: str) -> str:
    """Return the release tag encoded in a plain ``*.tar.aa`` archive-part basename."""
    if len(name) > 128:
        raise ValueError("archive part basename exceeds 128 characters")
    match = _PART.fullmatch(name)
    if match is None:
        raise ValueError(f"cannot derive adsb.lol release tag from archive part name {name!r}")
    tag = match.group("tag")
    _tag_day(tag)
    return tag


def release_tag_for_day(name: str, day: str) -> str:
    tag = release_tag(name)
    try:
        parsed_day = date.fromisoformat(day)
    except ValueError as exc:
        raise ValueError("package day must be a real YYYY-MM-DD calendar date") from exc
    if parsed_day.isoformat() != day or _tag_day(tag) != day:
        raise ValueError(f"archive part {name!r} is not for package UTC day {day}")
    return tag


def release_url(tag: str) -> str:
    _tag_day(tag)
    return f"{_RELEASE_ROOT}/tag/{tag}"


def download_url(tag: str, name: str) -> str:
    if release_tag(name) != tag:
        raise ValueError("archive filename does not belong to the declared adsb.lol release tag")
    return f"{_RELEASE_ROOT}/download/{tag}/{name}"


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "release-tag":
        try:
            print(release_tag(argv[1]))
            return 0
        except ValueError as exc:
            print(f"repro_zau_args: {exc}", file=sys.stderr)
            return 2
    print("usage: repro_zau_args.py release-tag PART_NAME", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
