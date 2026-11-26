import importlib.util
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib

SCRIPT = Path(__file__).parents[1] / "oaf_observer.py"
DISK_CHECK = Path(__file__).parents[1] / "check-disk.py"
SPEC = importlib.util.spec_from_file_location("oaf_observer", SCRIPT)
observer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(observer)


def frame(seq, recv_ns, kind=1, payload=b"sample", jms_ms=0):
    header = struct.pack("<4sBBH Q q q Q I", b"OAF2", kind, 0, 0,
                         seq, recv_ns, jms_ms, seq * 100, len(payload))
    body = header + payload
    return body + struct.pack("<I", zlib.crc32(body) & 0xffffffff)


class ObserverTests(unittest.TestCase):
    def test_observer_and_capture_have_independent_cursors_and_exact_window_spans(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            journal = root / "journal" / "SFDPS"
            journal.mkdir(parents=True)
            first = frame(0, 100_000_000_000, payload=b"one")
            middle = frame(1, 150_000_000_000, payload=b"two")
            last = frame(2, 200_000_000_000, payload=b"three")
            (journal / "SFDPS-20261130T000000Z-0001.oaf").write_bytes(first + middle + last)
            obs_state = root / "observer.json"
            cap_state = root / "capture.json"
            capture = {"from_ns": 150_000_000_000, "to_ns": 200_000_000_000,
                       "from_utc": "start", "to_utc": "end", "files": {}, "span_count": 0}
            state, _, count = observer.parse_cursor(journal, "SFDPS", obs_state, 60)
            self.assertEqual(count, 3)
            self.assertEqual(state["stats"]["message_frames"], 3)
            self.assertEqual(state["offset"], len(first + middle + last))
            archive, capture, cap_count = observer.parse_cursor(journal, "SFDPS", cap_state, 60, capture)
            self.assertEqual(cap_count, 3)
            self.assertEqual(archive["stats"]["message_frames"], 3)
            self.assertEqual(capture["files"]["SFDPS-20261130T000000Z-0001.oaf"]["frames"], 1)
            span = capture["files"]["SFDPS-20261130T000000Z-0001.oaf"]["spans"][0]
            self.assertEqual(span, {"start_offset": len(first), "end_offset": len(first) + len(middle), "frames": 1})
            self.assertNotEqual(obs_state, cap_state)

    def test_resume_reads_append_without_recounting_and_records_arrival_gap(self):
        with tempfile.TemporaryDirectory() as tmp:
            journal = Path(tmp) / "SFDPS"
            journal.mkdir()
            path = journal / "SFDPS-20261130T000000Z-0001.oaf"
            path.write_bytes(frame(0, 100_000_000_000) + frame(1, 101_000_000_000))
            cursor = Path(tmp) / "cursor.json"
            state, _, _ = observer.parse_cursor(journal, "SFDPS", cursor, 60)
            observer.atomic_json(cursor, state)
            path.write_bytes(path.read_bytes() + frame(2, 200_000_000_000, kind=3,
                                                       payload=b"state=disconnected"))
            state, _, count = observer.parse_cursor(journal, "SFDPS", cursor, 60)
            self.assertEqual(count, 1)
            self.assertEqual(state["stats"]["frames"], 3)
            self.assertEqual(state["stats"]["arrival_gaps_over_threshold"], 1)
            self.assertEqual(state["stats"]["status_disconnected"], 1)

    def test_capture_bounds_match_the_declared_nov30_acquisition_and_scored_split(self):
        start = observer.utc_ns("2026-11-30T05:00:00Z")
        score_start = observer.utc_ns("2026-11-30T06:00:00Z")
        score_end = observer.utc_ns("2026-12-01T00:00:00Z")
        end = observer.utc_ns("2026-12-01T06:00:00Z")
        self.assertEqual(score_start - start, 3600 * 1_000_000_000)
        self.assertEqual(score_end - score_start, 18 * 3600 * 1_000_000_000)
        self.assertEqual(end - score_end, 6 * 3600 * 1_000_000_000)

    def test_disk_preflight_refuses_latched_or_capped_feed_before_consume(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            journal = root / "journal"
            state = root / "state"
            for feed in observer.FEEDS:
                (journal / feed).mkdir(parents=True)
            state.mkdir()
            env = {**os.environ, "OA_JOURNAL_ROOT": str(journal),
                   "OA_STATE_ROOT": str(state), "OA_JOURNAL_MAX_BYTES": "1024",
                   "OA_DISK_RESERVE_BYTES": "0"}
            good = subprocess.run([sys.executable, str(DISK_CHECK), "SFDPS"], env=env,
                                  capture_output=True, text=True, check=False)
            self.assertEqual(good.returncode, 0, good.stderr)
            (journal / "TBFM" / "TBFM-test.oaf").write_bytes(b"x" * 1024)
            capped = subprocess.run([sys.executable, str(DISK_CHECK), "TBFM"], env=env,
                                    capture_output=True, text=True, check=False)
            self.assertNotEqual(capped.returncode, 0)
            self.assertIn("refusing to consume", capped.stderr)
            (state / "SFDPS.recording-failed").write_text("synthetic latch\n")
            latched = subprocess.run([sys.executable, str(DISK_CHECK), "SFDPS"], env=env,
                                     capture_output=True, text=True, check=False)
            self.assertNotEqual(latched.returncode, 0)
            self.assertIn("latched", latched.stderr)

    def test_partial_tail_is_not_consumed_until_rotation_then_counted_as_truncated(self):
        with tempfile.TemporaryDirectory() as tmp:
            journal = Path(tmp) / "TBFM"
            journal.mkdir()
            old = journal / "TBFM-20261130T000000Z-0001.oaf"
            old.write_bytes(frame(0, 100_000_000_000) + b"OAF")
            cursor = Path(tmp) / "cursor.json"
            state, _, _ = observer.parse_cursor(journal, "TBFM", cursor, 60)
            observer.atomic_json(cursor, state)
            self.assertEqual(state["offset"], len(frame(0, 100_000_000_000)))
            (journal / "TBFM-20261130T000001Z-0001.oaf").write_bytes(frame(1, 101_000_000_000))
            state, _, _ = observer.parse_cursor(journal, "TBFM", cursor, 60)
            self.assertEqual(state["stats"]["truncated_closed_segments"], 1)
            self.assertEqual(state["stats"]["message_frames"], 2)


if __name__ == "__main__":
    unittest.main()
