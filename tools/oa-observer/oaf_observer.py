#!/usr/bin/env python3
"""Bounded local observer for raw OAF2 segment journals. Never opens credentials or decodes payloads."""
from __future__ import annotations

import argparse
import datetime as dt
import fcntl
import json
import os
from pathlib import Path
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import time
import zlib

FEEDS = ("SFDPS", "STDDS", "TBFM")
MAGIC = b"OAF2"
HEADER = 44
TRAILER = 4
MAX_PAYLOAD = 4 * 1024 * 1024
MAX_FRAME = HEADER + MAX_PAYLOAD + TRAILER
MAX_FILES = 16384
MAX_BYTES_PER_POLL = 16 * 1024 * 1024
MAX_FRAMES_PER_POLL = 10000
MAX_CAPTURE_SPANS = 20000
POLL_SECONDS = 2
GAP_SECONDS = 60
STOP = False


class CaptureSpanLimitError(RuntimeError):
    pass


def stop(_signum: int, _frame: object) -> None:
    global STOP
    STOP = True


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            json.dump(value, output, sort_keys=True, separators=(",", ":"))
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        dirfd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dirfd)
        finally:
            os.close(dirfd)
    except BaseException:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise


def read_json(path: Path, default: dict) -> dict:
    try:
        st = path.lstat()
        if not stat.S_ISREG(st.st_mode) or st.st_size > 16 * 1024 * 1024:
            raise ValueError(f"unsafe or oversized cursor file: {path.name}")
        with path.open("r", encoding="utf-8") as source:
            value = json.load(source)
        if not isinstance(value, dict):
            raise ValueError(f"invalid cursor data: {path.name}")
        return value
    except FileNotFoundError:
        return default


def utc_ns(text: str) -> int:
    value = dt.datetime.fromisoformat(text.replace("Z", "+00:00"))
    if value.tzinfo is None or value.utcoffset() != dt.timedelta(0):
        raise argparse.ArgumentTypeError("capture bounds must be explicit UTC, e.g. 2026-11-30T00:00:00Z")
    return int(value.timestamp() * 1_000_000_000)


def segment_names(directory: Path, feed: str) -> list[str]:
    try:
        entries = list(os.scandir(directory))
    except FileNotFoundError:
        return []
    if len(entries) > MAX_FILES:
        raise RuntimeError(f"{feed}: journal has {len(entries)} entries; limit is {MAX_FILES}")
    names = []
    for entry in entries:
        if not entry.name.startswith(f"{feed}-") or not entry.name.endswith(".oaf"):
            continue
        if not entry.is_file(follow_symlinks=False):
            raise RuntimeError(f"{feed}: refusing non-regular journal entry {entry.name!r}")
        names.append(entry.name)
    return sorted(names)


def open_regular(path: Path) -> int:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    fd = os.open(path, flags)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise RuntimeError(f"refusing non-regular segment {path.name!r}")
    return fd


def fresh_counts() -> dict:
    return {
        "frames": 0, "message_frames": 0, "status_frames": 0, "reject_frames": 0,
        "status_connected": 0, "status_disconnected": 0, "status_reconnect_attempt": 0,
        "unknown_kind_frames": 0, "crc_errors": 0, "malformed_bytes": 0,
        "truncated_closed_segments": 0,
        "missing_segments": 0, "sequence_gaps": 0, "sequence_resets": 0,
        "receive_time_regressions": 0, "arrival_gaps_over_threshold": 0,
        "arrival_gap_seconds_total": 0, "arrival_gap_intervals_recent": [],
        "arrival_gap_intervals_dropped": 0, "adapter_status_events_recent": [],
        "adapter_status_events_dropped": 0, "jms_delta_samples": 0,
        "jms_delta_over_60s": 0, "jms_delta_min_ms": None, "jms_delta_max_ms": None,
        "first_receive_ns": None, "last_receive_ns": None,
        "last_sequence": None, "last_file": None, "last_offset": 0,
        "updated_utc": None,
    }


def add_frame(stats: dict, kind: int, seq: int, recv_ns: int, jms_ms: int,
              payload: bytes, file_name: str, end_offset: int, gap_seconds: int) -> None:
    stats["frames"] += 1
    key = {1: "message_frames", 2: "reject_frames", 3: "status_frames"}.get(kind)
    if key:
        stats[key] += 1
    else:
        stats["unknown_kind_frames"] += 1
    if kind == 3 and len(payload) <= 4096:
        text = payload.decode("ascii", errors="ignore")
        state_name = None
        if "state=connected" in text:
            stats["status_connected"] += 1
            state_name = "connected"
        elif "state=disconnected" in text:
            stats["status_disconnected"] += 1
            state_name = "disconnected"
        elif "state=reconnect_attempt" in text:
            stats["status_reconnect_attempt"] += 1
            state_name = "reconnect_attempt"
        if state_name is not None:
            events = stats["adapter_status_events_recent"]
            if len(events) >= 100:
                events.pop(0)
                stats["adapter_status_events_dropped"] += 1
            events.append({"state": state_name, "receive_ns": recv_ns})
    previous_seq = stats.get("last_sequence")
    if previous_seq is not None:
        if seq > previous_seq + 1:
            stats["sequence_gaps"] += seq - previous_seq - 1
        elif seq <= previous_seq:
            stats["sequence_resets"] += 1
    previous_recv = stats.get("last_receive_ns")
    if previous_recv is not None:
        delta = recv_ns - previous_recv
        if delta < 0:
            stats["receive_time_regressions"] += 1
        elif delta > gap_seconds * 1_000_000_000:
            stats["arrival_gaps_over_threshold"] += 1
            stats["arrival_gap_seconds_total"] += delta // 1_000_000_000
            intervals = stats["arrival_gap_intervals_recent"]
            if len(intervals) >= 100:
                intervals.pop(0)
                stats["arrival_gap_intervals_dropped"] += 1
            intervals.append({"from_receive_ns": previous_recv, "to_receive_ns": recv_ns,
                              "gap_seconds": delta // 1_000_000_000})
    if stats["first_receive_ns"] is None:
        stats["first_receive_ns"] = recv_ns
    stats["last_receive_ns"] = recv_ns
    stats["last_sequence"] = seq
    stats["last_file"] = file_name
    stats["last_offset"] = end_offset
    if kind == 1 and jms_ms > 0:
        # Diagnostic only: JMS timestamp semantics are not asserted to be source-event time.
        delta_ms = recv_ns // 1_000_000 - jms_ms
        stats["jms_delta_samples"] += 1
        if delta_ms > 60_000:
            stats["jms_delta_over_60s"] += 1
        low = stats["jms_delta_min_ms"]
        high = stats["jms_delta_max_ms"]
        stats["jms_delta_min_ms"] = delta_ms if low is None else min(low, delta_ms)
        stats["jms_delta_max_ms"] = delta_ms if high is None else max(high, delta_ms)


def parse_cursor(directory: Path, feed: str, cursor_path: Path, gap_seconds: int,
                 capture: dict | None = None) -> tuple[dict, dict | None, int]:
    state = read_json(cursor_path, {"file": None, "offset": 0, "stats": fresh_counts()})
    stats = state.get("stats")
    if not isinstance(stats, dict) or "frames" not in stats:
        raise RuntimeError(f"{feed}: cursor state is malformed")
    stats = {**fresh_counts(), **stats}
    names = segment_names(directory, feed)
    if not names:
        state["stats"] = stats
        return state, capture, 0

    current = state.get("file")
    offset = state.get("offset", 0)
    if not isinstance(offset, int) or offset < 0:
        raise RuntimeError(f"{feed}: invalid cursor offset")
    if current is None:
        index = 0
    elif current in names:
        index = names.index(current)
    else:
        later = [i for i, name in enumerate(names) if name > current]
        stats["missing_segments"] += 1
        index = later[0] if later else len(names)
        current = None
        offset = 0

    bytes_left = MAX_BYTES_PER_POLL
    frames_done = 0
    while index < len(names) and bytes_left > 0 and frames_done < MAX_FRAMES_PER_POLL:
        name = names[index]
        path = directory / name
        fd = open_regular(path)
        try:
            size = os.fstat(fd).st_size
            original_offset = offset
            remaining = max(0, size - offset)
            read_size = min(remaining, bytes_left)
            block = os.pread(fd, read_size, offset) if read_size else b""
            bytes_left -= len(block)
            pos = 0
            while pos + 4 <= len(block) and frames_done < MAX_FRAMES_PER_POLL:
                if block[pos:pos + 4] != MAGIC:
                    found = block.find(MAGIC, pos + 1)
                    if found < 0:
                        discard = max(0, len(block) - pos - 3)
                        stats["malformed_bytes"] += discard
                        pos += discard
                        break
                    stats["malformed_bytes"] += found - pos
                    pos = found
                    continue
                if len(block) - pos < HEADER:
                    break
                payload_len = struct.unpack_from("<I", block, pos + 40)[0]
                if payload_len > MAX_PAYLOAD:
                    stats["malformed_bytes"] += 1
                    pos += 1
                    continue
                total = HEADER + payload_len + TRAILER
                if len(block) - pos < total:
                    break
                frame = block[pos:pos + total]
                expected = struct.unpack_from("<I", frame, total - TRAILER)[0]
                if zlib.crc32(frame[:-TRAILER]) & 0xFFFFFFFF != expected:
                    stats["crc_errors"] += 1
                    pos += 1
                    continue
                kind = frame[4]
                seq = struct.unpack_from("<Q", frame, 8)[0]
                recv_ns = struct.unpack_from("<q", frame, 16)[0]
                jms_ms = struct.unpack_from("<q", frame, 24)[0]
                start = offset + pos
                end = start + total
                payload = frame[HEADER:HEADER + payload_len]
                add_frame(stats, kind, seq, recv_ns, jms_ms, payload, name, end, gap_seconds)
                if capture is not None and capture["from_ns"] <= recv_ns < capture["to_ns"]:
                    rows = capture["files"].setdefault(name, {"spans": [], "frames": 0,
                        "first_receive_ns": recv_ns, "last_receive_ns": recv_ns})
                    spans = rows["spans"]
                    if spans and spans[-1]["end_offset"] == start:
                        spans[-1]["end_offset"] = end
                        spans[-1]["frames"] += 1
                    else:
                        if capture["span_count"] >= MAX_CAPTURE_SPANS:
                            raise CaptureSpanLimitError(
                                "capture manifest span limit exceeded; refusing truncation")
                        spans.append({"start_offset": start, "end_offset": end, "frames": 1})
                        capture["span_count"] += 1
                    rows["frames"] += 1
                    rows["first_receive_ns"] = min(rows["first_receive_ns"], recv_ns)
                    rows["last_receive_ns"] = max(rows["last_receive_ns"], recv_ns)
                pos += total
                frames_done += 1
            offset = original_offset + pos
            if read_size < remaining:
                # Budget ended before EOF; retain this file cursor for the next poll.
                current = name
                break
            if frames_done >= MAX_FRAMES_PER_POLL and pos < len(block):
                current = name
                break
            if index + 1 < len(names):
                if pos < len(block):
                    # A later rotated segment proves this one is closed; a partial tail is damaged.
                    stats["truncated_closed_segments"] += 1
                    stats["malformed_bytes"] += len(block) - pos
                index += 1
                offset = 0
                current = None
                continue
            current = name
            break
        finally:
            os.close(fd)
    stats["updated_utc"] = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    next_file = current if current is not None else names[min(index, len(names) - 1)]
    return {"file": next_file, "offset": offset, "stats": stats}, capture, frames_done


def backlog_bytes(directory: Path, feed: str, state: dict) -> int:
    names = segment_names(directory, feed)
    current = state.get("file")
    if current not in names:
        return sum((directory / name).stat().st_size for name in names)
    index = names.index(current)
    total = max(0, (directory / current).stat().st_size - state.get("offset", 0))
    for name in names[index + 1:]:
        total += (directory / name).stat().st_size
    return total


def systemd_uptime(service: str) -> str | None:
    try:
        result = subprocess.run(["systemctl", "--user", "show", service,
                                 "--property=ActiveEnterTimestamp", "--value"],
                                capture_output=True, text=True, timeout=2, check=False)
        value = result.stdout.strip()
        return value if result.returncode == 0 and value and value != "n/a" else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def ns_iso(value: int | None) -> str | None:
    if value is None:
        return None
    return dt.datetime.fromtimestamp(value / 1_000_000_000, dt.timezone.utc).isoformat().replace("+00:00", "Z")


def load_capture_failure(path: Path, capture: dict | None) -> dict | None:
    if capture is None:
        return None
    failure = read_json(path, {})
    if not failure:
        return None
    if (failure.get("from_ns") != capture["from_ns"]
            or failure.get("to_ns") != capture["to_ns"]):
        raise ValueError("capture failure latch bounds differ; preserve failed archive and use a new state directory")
    return failure


def capture_processing_allowed(capture: dict | None, failure: dict | None) -> bool:
    return capture is not None and failure is None


def latch_capture_failure(path: Path, capture: dict, feed: str, reason: str) -> dict:
    failure = {"schema": "openatca-capture-failure-v1", "status": "FAILED_INCOMPLETE",
               "from_ns": capture["from_ns"], "to_ns": capture["to_ns"],
               "from_utc": capture["from_utc"], "to_utc": capture["to_utc"],
               "feed": feed, "reason": reason,
               "failed_utc": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
               "operator_action": "preserve this failure record and partial manifest; do not resume or claim completion"}
    atomic_json(path, failure)
    return failure


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--journal-root", type=Path, required=True)
    parser.add_argument("--state-root", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--capture-manifest", type=Path, required=True)
    parser.add_argument("--capture-pin", type=Path, required=True)
    parser.add_argument("--capture-from-utc")
    parser.add_argument("--capture-to-utc")
    parser.add_argument("--gap-seconds", type=int, default=GAP_SECONDS)
    parser.add_argument("--poll-seconds", type=int, default=POLL_SECONDS)
    parser.add_argument("--duration-seconds", type=int, default=30 * 24 * 60 * 60)
    args = parser.parse_args()
    if args.gap_seconds < 1 or args.poll_seconds < 1 or args.duration_seconds < 1:
        parser.error("gap, poll and duration must be positive")
    if bool(args.capture_from_utc) != bool(args.capture_to_utc):
        parser.error("capture window needs both --capture-from-utc and --capture-to-utc")
    capture = None
    if args.capture_from_utc:
        try:
            start, end = utc_ns(args.capture_from_utc), utc_ns(args.capture_to_utc)
        except (ValueError, OverflowError) as exc:
            parser.error(str(exc))
        if start >= end:
            parser.error("capture window must be non-empty [from,to)")
        capture = {"from_ns": start, "to_ns": end, "from_utc": args.capture_from_utc,
                   "to_utc": args.capture_to_utc}
        old = read_json(args.capture_manifest, {})
        if old and (old.get("from_ns") != start or old.get("to_ns") != end):
            parser.error("capture bounds differ from existing manifest; use a new archive state")

    args.state_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        capture_failure = load_capture_failure(args.state_root / "capture-failed.json", capture)
    except ValueError as exc:
        parser.error(str(exc))
    lock_path = args.state_root / "observer.lock"
    lock_fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("observer already holds the state lock", file=sys.stderr)
        return 2

    run_path = args.state_root / "run.json"
    run_state = read_json(run_path, {})
    now_utc = dt.datetime.now(dt.timezone.utc)
    if not run_state:
        end_utc = now_utc + dt.timedelta(seconds=args.duration_seconds)
        run_state = {"schema": "openatca-observer-run-v1",
                     "started_utc": now_utc.isoformat().replace("+00:00", "Z"),
                     "deadline_utc": end_utc.isoformat().replace("+00:00", "Z"),
                     "duration_seconds": args.duration_seconds}
        atomic_json(run_path, run_state)
    try:
        deadline_utc = dt.datetime.fromisoformat(run_state["deadline_utc"].replace("Z", "+00:00"))
    except (KeyError, ValueError, TypeError) as exc:
        raise SystemExit(f"invalid persisted observer deadline: {exc}")
    invocation_started_utc = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    started_mono = time.monotonic()
    remaining_at_start = max(0, int((deadline_utc - now_utc).total_seconds()))
    mono_deadline = started_mono + remaining_at_start
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    while not STOP and time.monotonic() < mono_deadline:
        report = {"schema": "openatca-local-observer-v1", "observer_started_utc": run_state["started_utc"],
                  "process_invocation_started_utc": invocation_started_utc,
                  "observer_deadline_utc": run_state["deadline_utc"],
                  "observer_uptime_seconds": int(args.duration_seconds - max(0, (deadline_utc - dt.datetime.now(dt.timezone.utc)).total_seconds())),
                  "capture_semantics": "OAF2 frame receipt-time window [from,to); payload not decoded; source-event lateness unavailable",
                  "feeds": {}}
        try:
            for feed in FEEDS:
                directory = args.journal_root / feed
                cursor_file = args.state_root / "observer" / f"{feed}.json"
                state, _, _ = parse_cursor(directory, feed, cursor_file, args.gap_seconds)
                atomic_json(cursor_file, state)
                stats = state["stats"]
                last = stats.get("last_receive_ns")
                silence = None if last is None else max(0, time.time_ns() - last) // 1_000_000_000
                report["feeds"][feed] = {
                    **stats,
                    "first_receive_utc": ns_iso(stats.get("first_receive_ns")),
                    "last_receive_utc": ns_iso(last),
                    "arrival_silence_seconds": silence,
                    "unread_journal_bytes_at_poll": backlog_bytes(directory, feed, state),
                    "collector_systemd_active_since": systemd_uptime(f"openatca-collector@{feed}.service"),
                    "recording_failure_latched": (args.state_root / f"{feed}.recording-failed").exists(),
                    "jms_delta_note": "receipt time minus JMS timestamp only; semantics unknown, not source-event latency",
                    "outage_note": "STATUS disconnected/reconnect counts indicate adapter-reported interruption; silence alone is not proof of broker/feed outage",
            "availability_note": "counts are OAF frame availability at this recorder; source-event availability and message semantics are not decoded",
                }
            if capture_processing_allowed(capture, capture_failure):
                capture_progress = {}
                captured_files = {}
                span_count = 0
                for feed in FEEDS:
                    path = args.state_root / "capture" / f"{feed}.json"
                    prior = read_json(path, {"file": None, "offset": 0, "stats": fresh_counts()})
                    feed_capture = {**capture,
                        "files": prior.get("capture_files", {}),
                        "span_count": prior.get("capture_span_count", 0)}
                    if prior.get("capture_from_ns", capture["from_ns"]) != capture["from_ns"] or prior.get("capture_to_ns", capture["to_ns"]) != capture["to_ns"]:
                        raise RuntimeError(f"{feed}: capture cursor bounds changed; use a new capture state directory")
                    try:
                        state, feed_capture, _ = parse_cursor(args.journal_root / feed, feed, path,
                                                              args.gap_seconds, feed_capture)
                    except CaptureSpanLimitError as exc:
                        capture_failure = latch_capture_failure(
                            args.state_root / "capture-failed.json", capture, feed, str(exc))
                        previous_manifest = read_json(args.capture_manifest, {})
                        previous_manifest.update({"schema": "openatca-capture-window-manifest-v1",
                            "from_utc": capture["from_utc"], "to_utc": capture["to_utc"],
                            "from_ns": capture["from_ns"], "to_ns": capture["to_ns"],
                            "status": "FAILED_INCOMPLETE; no completeness claim",
                            "failure": capture_failure,
                            "updated_utc": capture_failure["failed_utc"]})
                        atomic_json(args.capture_manifest, previous_manifest)
                        print(f"capture failed and latched: feed={feed}; {exc}",
                              file=sys.stderr, flush=True)
                        break
                    state["capture_from_ns"] = capture["from_ns"]
                    state["capture_to_ns"] = capture["to_ns"]
                    state["capture_files"] = feed_capture["files"]
                    state["capture_span_count"] = feed_capture["span_count"]
                    atomic_json(path, state)
                    captured_files[feed] = feed_capture["files"]
                    span_count += feed_capture["span_count"]
                    names = segment_names(args.journal_root / feed, feed)
                    current = state.get("file")
                    backlog = 0
                    if current in names:
                        idx = names.index(current)
                        current_size = (args.journal_root / feed / current).stat().st_size
                        backlog += max(0, current_size - state.get("offset", 0))
                        for later in names[idx + 1:]:
                            backlog += (args.journal_root / feed / later).stat().st_size
                    capture_progress[feed] = {"frames_scanned": state["stats"]["frames"],
                        "last_file": state.get("file"), "last_offset": state.get("offset"),
                        "last_receive_utc": ns_iso(state["stats"].get("last_receive_ns")),
                        "unread_journal_bytes_at_poll": backlog}
                if capture_failure is None:
                    manifest = {"schema": "openatca-capture-window-manifest-v1",
                    "from_utc": capture["from_utc"], "to_utc": capture["to_utc"],
                    "from_ns": capture["from_ns"], "to_ns": capture["to_ns"],
                    "selection": "complete original OAF2 frames with recorder receive timestamp in [from,to); all frame kinds; payload bytes are not copied or changed",
                    "p010_protocol_windows_utc": {
                        "warmup_excluded_from_scoring": ["2026-11-30T05:00:00Z", "2026-11-30T06:00:00Z"],
                        "scored_interval": ["2026-11-30T06:00:00Z", "2026-12-01T00:00:00Z"],
                        "acquisition_after_scoring": ["2026-12-01T00:00:00Z", "2026-12-01T06:00:00Z"]},
                    "source_journal_root": str(args.journal_root), "retention": "segments are append-only; collector has a total byte cap and does not delete; this pin records required source spans",
                    "status": "IN_PROGRESS; no completeness claim until a separate close check",
                    "cursor_progress": capture_progress, "span_count": span_count,
                    "files_by_feed": captured_files,
                    "updated_utc": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")}
                    atomic_json(args.capture_manifest, manifest)
                    atomic_json(args.capture_pin, {"schema": "openatca-journal-retention-pin-v1",
                    "from_utc": capture["from_utc"], "to_utc": capture["to_utc"],
                    "manifest": str(args.capture_manifest),
                    "files_by_feed": {feed: sorted(captured_files[feed]) for feed in FEEDS},
                    "policy": "do not delete referenced source segments until this capture is separately acknowledged/released"})
            if capture_failure is not None:
                report["capture_cursor"] = "FAILED_INCOMPLETE; operator action required; observer continues without capture retries"
                report["capture_failure"] = capture_failure
            else:
                report["capture_cursor"] = "active" if capture is not None else "not-configured; install a systemd drop-in with explicit UTC bounds"
            atomic_json(args.report, report)
            print(json.dumps({"observer_uptime_seconds": report["observer_uptime_seconds"],
                              "feed_frames": {f: report["feeds"][f]["frames"] for f in FEEDS},
                              "capture_cursor": report["capture_cursor"]}, separators=(",", ":")), flush=True)
        except (OSError, RuntimeError, ValueError, json.JSONDecodeError) as exc:
            print(f"observer refused: {exc}", file=sys.stderr, flush=True)
            return 1
        time.sleep(min(args.poll_seconds, max(0, mono_deadline - time.monotonic())))
    final_report = read_json(args.report, {})
    final_report["observer_ended_utc"] = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    final_report["observer_status"] = (
        "SCHEDULED_30_DAY_WINDOW_COMPLETE"
        if dt.datetime.now(dt.timezone.utc) >= deadline_utc
        else "STOPPED_BEFORE_SCHEDULED_END; inspect service history"
    )
    atomic_json(args.report, final_report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
