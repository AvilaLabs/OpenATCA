#!/usr/bin/env python3
"""Bounded, private development capture of adsb.lol regional V2 snapshots.

This tool never scores data. It retains each exact HTTPS response before decoding it, streams
OAF2 MESSAGE/ADS3 frames from the sibling oa-adsb-snapshot executable, and publishes COMPLETE
only after every response, output, and manifest has been flushed and hashed. Do not use these
snapshot-derived timestamp estimates as precision benchmark truth without a separate evaluation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import stat
import sys
import time
from datetime import datetime, timezone
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tools"))
import sealed_guard  # noqa: E402

MAX_DURATION_S = 300
MIN_POLL_S = 5
MAX_POLL_S = 300
MAX_RESPONSES = 60
MAX_BODY_BYTES = 1_048_576
MAX_TOTAL_BODY_BYTES = 64 * 1024 * 1024
MAX_TOTAL_OAF_BYTES = 256 * 1024 * 1024
MAX_AGE_MS = 30_000
UA = "OpenATCA-development-capture/1.0 (private research; contact Connor Avila)"


def utc_ms() -> int:
    return time.time_ns() // 1_000_000


def utc_text(ms: int | None = None) -> str:
    if ms is None:
        ms = utc_ms()
    return datetime.fromtimestamp(ms / 1000, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def create_exclusive(path: Path, data: bytes) -> None:
    guard = sealed_guard.production()
    guard.check(str(path))
    made = False
    try:
        with path.open("xb") as f:
            made = True
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
    except BaseException:
        if made:
            guard.check(str(path))
            path.unlink(missing_ok=True)
        raise


def request_url(lat: float, lon: float, radius_nm: int) -> str:
    return ("https://api.adsb.lol/v2/lat/" + quote(f"{lat:.5f}", safe="") + "/lon/"
            + quote(f"{lon:.5f}", safe="") + "/dist/" + str(radius_nm))


def metar_url(stations: str) -> str:
    return "https://aviationweather.gov/api/data/metar?ids=" + quote(stations, safe=",") + "&format=json"


def fetch_once(url: str, timeout_s: int, *, allow_204: bool = False) -> tuple[bytes, dict[str, str], int, int, int]:
    request = Request(url, headers={"Accept": "application/json", "User-Agent": UA})
    try:
        with urlopen(request, timeout=timeout_s) as response:
            if response.status != 200 and not (allow_204 and response.status == 204):
                raise RuntimeError(f"adsb.lol HTTP status {response.status}")
            body = response.read(MAX_BODY_BYTES + 1)
            headers = {key: response.headers.get(key, "") for key in ("Date", "ETag", "Last-Modified", "Content-Type")}
            status = response.status
    except HTTPError as e:
        raise RuntimeError(f"HTTP status {e.code}; Retry-After={e.headers.get('Retry-After', '')}") from e
    except URLError as e:
        raise RuntimeError(f"adsb.lol request failed: {e.reason}") from e
    received_mono = time.monotonic_ns()
    received_ms = utc_ms()
    if len(body) > MAX_BODY_BYTES:
        raise RuntimeError(f"response exceeds {MAX_BODY_BYTES} bytes")
    return body, headers, status, received_ms, received_mono


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--out-dir", required=True, type=Path)
    p.add_argument("--decoder", required=True, type=Path, help="prebuilt oa-adsb-snapshot executable")
    p.add_argument("--cohort-from-utc", required=True)
    p.add_argument("--cohort-to-utc", required=True)
    p.add_argument("--lat", type=float, default=42.5)
    p.add_argument("--lon", type=float, default=-88.5)
    p.add_argument("--radius-nm", type=int, default=250)
    p.add_argument("--duration-s", type=int, default=300)
    p.add_argument("--poll-s", type=int, default=5)
    p.add_argument("--timeout-s", type=int, default=10)
    p.add_argument("--max-age-ms", type=int, default=15_000)
    p.add_argument("--metar-stations", default="KORD,KMDW,KBMI,KIND,KMSN")
    return p.parse_args()


def parse_utc(text: str) -> int:
    value = datetime.fromisoformat(text.replace("Z", "+00:00"))
    if value.tzinfo is None:
        raise ValueError("UTC bounds need an explicit Z or offset")
    return int(value.astimezone(timezone.utc).timestamp() * 1000)


def checked_options(a: argparse.Namespace) -> tuple[int, int]:
    if not 1 <= a.duration_s <= MAX_DURATION_S:
        raise ValueError(f"duration must be 1 to {MAX_DURATION_S} seconds")
    if not MIN_POLL_S <= a.poll_s <= MAX_POLL_S:
        raise ValueError(f"poll interval must be {MIN_POLL_S} to {MAX_POLL_S} seconds")
    if not 1 <= a.timeout_s <= 15:
        raise ValueError("HTTP timeout must be 1 to 15 seconds")
    if not 1 <= a.max_age_ms <= MAX_AGE_MS:
        raise ValueError(f"max age must be 1 to {MAX_AGE_MS} milliseconds")
    if not -90 <= a.lat <= 90 or not -180 <= a.lon <= 180 or not 1 <= a.radius_nm <= 250:
        raise ValueError("invalid query center/radius; API radius is 1 to 250 nm")
    start, end = parse_utc(a.cohort_from_utc), parse_utc(a.cohort_to_utc)
    if not start < end:
        raise ValueError("cohort from bound must be before to bound")
    if a.duration_s * 1000 > end - start:
        raise ValueError("capture duration exceeds the preregistered cohort interval")
    stations = a.metar_stations.split(",")
    if any(len(s) != 4 or not s.isascii() or not s.isalpha() or s.upper() != s for s in stations):
        raise ValueError("METAR stations must be comma-separated uppercase four-letter identifiers")
    return start, end


def main() -> int:
    a = parse_args()
    guard = sealed_guard.production()
    reserved_out: Path | None = None
    entries: list[dict[str, object]] = []
    raw_files: list[dict[str, object]] = []
    total_body = total_oaf = requests = frame_seq = 0
    oaf_digest = hashlib.sha256()
    complete_written = False
    try:
        cohort_from, cohort_to = checked_options(a)
        guard.check(str(a.out_dir))
        guard.check(str(a.decoder))
        private_path = a.out_dir.resolve()
        if "openatca-private" not in private_path.parts:
            raise RuntimeError("capture outputs must be stored below an openatca-private directory")
        if a.out_dir.exists() or a.out_dir.is_symlink():
            raise RuntimeError("output directory exists; refusing overwrite")
        if not a.decoder.is_file() or not os.access(a.decoder, os.X_OK):
            raise RuntimeError("decoder is not an executable file")
        decoder_stat = os.lstat(a.decoder)
        if not stat.S_ISREG(decoder_stat.st_mode) or decoder_stat.st_nlink != 1:
            raise RuntimeError("decoder must be a single-link regular file")
        if sealed_guard.getsize(str(a.decoder)) <= 0:
            raise RuntimeError("decoder executable is empty")
        decoder_hash = digest(a.decoder)
        a.out_dir.mkdir(mode=0o700)
        reserved_out = a.out_dir
        start_wall, start_mono = utc_ms(), time.monotonic_ns()
        if start_wall < cohort_from:
            raise RuntimeError("capture start precedes the preregistered cohort interval")
        if start_wall >= cohort_to:
            raise RuntimeError("capture start is at or beyond the preregistered cohort end")
        responses = a.out_dir / "responses"
        guard.check(str(responses))
        responses.mkdir(mode=0o700)
        part = a.out_dir / "aircraft.oaf.part"
        guard.check(str(part))
        stop_at = start_mono + a.duration_s * 1_000_000_000
        metar_body, metar_headers, metar_status, metar_ms, metar_mono = fetch_once(metar_url(a.metar_stations), a.timeout_s, allow_204=True)
        metar_path = responses / "metar-current.json"
        create_exclusive(metar_path, metar_body)
        metar_record = {"source": "official Aviation Weather Center Data API", "url": metar_url(a.metar_stations),
                        "stations": a.metar_stations, "http_status": metar_status,
                        "received_utc": utc_text(metar_ms), "received_unix_ms": metar_ms,
                        "received_monotonic_ns": metar_mono, "headers": metar_headers,
                        "raw_file": "responses/metar-current.json", "raw_bytes": len(metar_body),
                        "raw_sha256": hashlib.sha256(metar_body).hexdigest()}
        total_body += len(metar_body)
        raw_files.append({"file": "responses/metar-current.json", "bytes": len(metar_body), "sha256": hashlib.sha256(metar_body).hexdigest()})
        if total_body > MAX_TOTAL_BODY_BYTES:
            raise RuntimeError(f"cumulative response bytes exceed {MAX_TOTAL_BODY_BYTES}")
        url = request_url(a.lat, a.lon, a.radius_nm)
        part_fd = os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            with os.fdopen(part_fd, "wb") as clip:
                while time.monotonic_ns() < stop_at and requests < MAX_RESPONSES:
                    if utc_ms() >= cohort_to:
                        raise RuntimeError("capture reached the preregistered cohort end")
                    body, headers, status, receipt_ms, receipt_mono = fetch_once(url, a.timeout_s)
                    requests += 1
                    total_body += len(body)
                    if total_body > MAX_TOTAL_BODY_BYTES:
                        raise RuntimeError(f"cumulative response bytes exceed {MAX_TOTAL_BODY_BYTES}")
                    name = f"snapshot-{requests:03}.json"
                    raw = responses / name
                    create_exclusive(raw, body)
                    raw_files.append({"file": f"responses/{name}", "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest()})
                    seq = frame_seq
                    command = [str(a.decoder), "--received-unix-ms", str(receipt_ms), "--received-mono-ns", str(receipt_mono), "--seq", str(seq), "--max-age-ms", str(a.max_age_ms)]
                    proc = subprocess.run(command, input=body, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                          timeout=15, check=False, close_fds=True)
                    if proc.returncode != 0:
                        raise RuntimeError(f"snapshot decoder failed for {name}: {proc.stderr.decode('utf-8', 'replace')[-500:]}")
                    if total_oaf + len(proc.stdout) > MAX_TOTAL_OAF_BYTES:
                        raise RuntimeError(f"cumulative OAF bytes exceed {MAX_TOTAL_OAF_BYTES}")
                    clip.write(proc.stdout)
                    oaf_digest.update(proc.stdout)
                    clip.flush()
                    os.fsync(clip.fileno())
                    total_oaf += len(proc.stdout)
                    if proc.stdout:
                        frame_seq += 1
                    entries.append({"index": requests, "raw_file": f"responses/{name}", "http_status": status,
                                    "received_utc": utc_text(receipt_ms), "received_unix_ms": receipt_ms,
                                    "received_monotonic_ns": receipt_mono, "raw_bytes": len(body),
                                    "raw_sha256": hashlib.sha256(body).hexdigest(), "response_headers": headers,
                                    "decoder_argv": command, "decoder_stderr": proc.stderr.decode("utf-8", "replace").strip(),
                                    "oaf_frame_bytes": len(proc.stdout), "max_age_ms": a.max_age_ms})
                    wait_ns = a.poll_s * 1_000_000_000 - (time.monotonic_ns() - receipt_mono)
                    if wait_ns > 0 and time.monotonic_ns() + wait_ns < stop_at:
                        time.sleep(wait_ns / 1_000_000_000)
                clip.flush()
                os.fsync(clip.fileno())
        except BaseException:
            try:
                os.close(part_fd)
            except OSError:
                pass
            raise
        stop_wall, stop_mono = utc_ms(), time.monotonic_ns()
        if requests == 0:
            raise RuntimeError("no snapshots were captured")
        if stop_wall > cohort_to:
            raise RuntimeError("capture stop exceeded the preregistered cohort end")
        final_clip = a.out_dir / "aircraft.oaf"
        guard.check(str(final_clip))
        os.link(part, final_clip)
        part.unlink()
        tool_path = Path(__file__).resolve()
        guard.check(str(tool_path))
        manifest = {
            "schema": "openatca.adsb-live-capture.v1", "status": "complete",
            "source": "adsb.lol V2 regional JSON snapshot API; ODbL 1.0; each raw response retained unchanged",
            "endpoint": url, "request_user_agent": UA, "query": {"lat": a.lat, "lon": a.lon, "radius_nm": a.radius_nm},
            "capture_start_utc": utc_text(start_wall), "capture_stop_utc": utc_text(stop_wall),
            "capture_start_monotonic_ns": start_mono, "capture_stop_monotonic_ns": stop_mono,
            "duration_limit_s": a.duration_s, "poll_interval_s": a.poll_s, "http_timeout_s": a.timeout_s,
            "cohort_interval_utc": {"from_inclusive": a.cohort_from_utc, "to_exclusive": a.cohort_to_utc},
            "cohort_timestamp_policy": "ADS-B estimated report time is server now minus raw seen_pos; exact source response preserves age values; timestamp age/quantization uncertainty remains; not precision truth by default",
            "decoder_path": str(a.decoder), "decoder_sha256": decoder_hash,
            "capture_tool_path": str(tool_path), "capture_tool_sha256": digest(tool_path),
            "decoder_source": "oa-adsb-snapshot; existing ADS3 payload carried in OAF2 MESSAGE frames with local receipt wall and monotonic times",
            "caps": {"max_responses": MAX_RESPONSES, "max_body_bytes": MAX_BODY_BYTES, "max_total_body_bytes": MAX_TOTAL_BODY_BYTES,
                     "max_total_oaf_bytes": MAX_TOTAL_OAF_BYTES, "max_position_age_ms": MAX_AGE_MS},
            "totals": {"responses": requests, "body_bytes": total_body, "oaf_bytes": total_oaf},
            "snapshots": entries,
            "metar": metar_record,
            "output": {"file": "aircraft.oaf", "bytes": final_clip.stat().st_size, "sha256": digest(final_clip)},
            "terms": "Private development data only. Do not publish FAA-origin SWIM data or claim independent radar truth from ADS-B source flags.",
        }
        manifest_path = a.out_dir / "manifest.json"
        create_exclusive(manifest_path, (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode())
        marker = ("openatca.adsb-live-capture.complete.v1\nmanifest_sha256=" + digest(manifest_path) + "\n").encode()
        create_exclusive(a.out_dir / "COMPLETE", marker)
        complete_written = True
        dir_fd = os.open(a.out_dir, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, sealed_guard.SealedError) as e:
        if reserved_out is not None:
            try:
                guard.check(str(reserved_out))
                if complete_written:
                    complete_path = reserved_out / "COMPLETE"
                    guard.check(str(complete_path))
                    complete_path.unlink()
                marker = reserved_out / "INCOMPLETE.json"
                create_exclusive(marker, (json.dumps({"status": "incomplete", "reason": str(e), "recorded_utc": utc_text(),
                                                      "responses": requests, "response_files": raw_files,
                                                      "snapshots": entries, "body_bytes": total_body,
                                                      "oaf_bytes": total_oaf, "oaf_partial_sha256": oaf_digest.hexdigest()},
                                                     sort_keys=True) + "\n").encode())
            except (OSError, RuntimeError, sealed_guard.SealedError):
                pass
        print(f"capture_adsb_lol: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
