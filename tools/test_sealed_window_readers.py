"""The sealed time window in the Python tools: the text scan, the fuzz corpus builder and its
`--filter-existing` mode, and every reader that takes rows with times. Synthetic files in temp
directories only; nothing here reads a real data directory."""
import calendar
import csv
import gzip
import io
import json
import os
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard as sg  # noqa: E402
import comparison_baseline as cb  # noqa: E402
import comparison_evaluator as ce  # noqa: E402
import comparison_reference as ref  # noqa: E402
import motion_study as ms  # noqa: E402
import run_comparison as rc  # noqa: E402
import track_set_score as tss  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
NS = 1_000_000_000
# The list's window is 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z, widened 15 minutes each side.
IN_NS = calendar.timegm((2026, 11, 21, 12, 0, 0)) * NS
OUT_NS = calendar.timegm((2026, 11, 10, 12, 0, 0)) * NS


def sealed(text: str) -> bool:
    return sg.text_has_sealed_time(text.encode())


def frame(recv_ns, payload=b"hello", jms_ms=0, seq=1):
    return struct.pack("<4sBBHQqqI", b"OAF1", 1, 0, 0, seq, recv_ns, jms_ms, len(payload)) + payload + b"\0\0\0\0"


class TextScanTests(unittest.TestCase):
    # REQ: OA-SEAL-011
    def test_iso_forms_in_the_window_are_found(self):
        for t in ("2026-11-21T12:00:00Z", "2026-11-21T12:00:00.123456Z", "2026-11-21T12:00:00", "2026-11-21 12:00:00",
                  "2026-11-21T12:00Z", "20261121T120000Z", "20261121T1200Z", "2026-11-21T12:00:00+05:00",
                  "2026-11-21T12:00:00-0800", "2026-11-21", "20261121", "2026-11-23T00:20:00Z",
                  "2026-11-19T23:50:00Z", "202611211200", "20261121120000", "2026-11-21T24:00:00Z"):
            self.assertTrue(sealed(f"<a t='{t}'/>"), t)

    def test_times_outside_the_window_pass(self):
        for t in ("2026-11-10T12:00:00Z", "2026-11-19T23:30:00Z", "2026-11-23T00:40:00Z", "2026-12-01", "20261201T000000Z",
                  "2025-11-21T12:00:00Z", "no time here", "2026-11-18T12:00:00Z"):
            self.assertFalse(sealed(f"<a t='{t}'/>"), t)

    def test_an_offset_moves_the_instant(self):
        # window is [2026-11-19T23:45Z, 2026-11-23T00:32Z)
        self.assertTrue(sealed("2026-11-23T05:00:00+06:00"))   # 2026-11-22T23:00Z
        self.assertFalse(sealed("2026-11-23T05:40:00+05:00"))
        self.assertFalse(sealed("2026-11-19T22:00:00+01:00"))  # 21:00Z the day before
        self.assertTrue(sealed("2026-11-19T22:00:00-03:00"))   # 01:00Z on the 20th

    def test_date_only_is_the_whole_day(self):
        self.assertTrue(sealed("<d>2026-11-22</d>"))
        self.assertFalse(sealed("<d>2026-11-18</d>"))

    def test_character_references_are_decoded_first(self):
        self.assertTrue(sealed("2026&#45;11&#45;21T12&#58;00&#58;00Z"))
        self.assertTrue(sealed("&#x32;026-11-21T12:00:00Z"))
        self.assertTrue(sealed("&#38;#50;026-11-21"))

    def test_fail_closed(self):
        for bad in ("2026-13-45T25:61:61Z", "2026-02-30", "2026-11-21T12:00:00+99:99", "20261399",
                    "&nbsp;", "&#xZZ;", "&#99999999;", "&#0;", "&#12", "x&unknown;y"):
            self.assertTrue(sealed(bad), bad)
        self.assertFalse(sealed("a &amp; b &lt; c"))

    def test_epoch_times_and_encodings(self):
        s = IN_NS // NS
        for t in (str(s), str(s * 1000), str(s * 1_000_000), str(s * NS), f"{s}.5"):
            self.assertTrue(sealed(f"<t>{t}</t>"), t)
        self.assertFalse(sealed(f"<t>{OUT_NS // NS}</t>"))
        self.assertTrue(sg.text_has_sealed_time("2026-11-21T12:00:00Z".encode("utf-16-le")))
        self.assertTrue(sg.text_has_sealed_time(b"\xff\xfe" + "2026-11-21".encode("utf-16-le")))
        self.assertTrue(sg.text_has_sealed_time(b"\x00\x01\xfe" + b"2026-11-21T12:00:00Z" + b"\xff"))

    def test_a_list_that_does_not_parse_seals_everything(self):
        self.assertTrue(sg.text_has_sealed_time(b"nothing", sg.Guard(text="garbage")))


class RowTests(unittest.TestCase):
    def test_time_cells(self):
        self.assertTrue(sg.time_in_window(str(IN_NS)))
        self.assertFalse(sg.time_in_window(str(OUT_NS)))
        self.assertTrue(sg.time_in_window("12x"))
        self.assertTrue(sg.time_in_window(""))
        self.assertFalse(sg.time_in_window("", optional=True))
        self.assertTrue(sg.time_in_window(None))

    def test_drop_counter_line_is_printed_once_at_exit_even_after_an_error(self):
        code = ("import sys; sys.path.insert(0, %r); import sealed_guard as g; c = g.DropCounter(); c.add(); c.add(2); "
                "raise SystemExit(3)" % str(Path(__file__).resolve().parent))
        r = subprocess.run([sys.executable, "-I", "-c", code], capture_output=True, text=True)
        self.assertEqual(r.returncode, 3)
        self.assertEqual(r.stderr.count("sealed-window rows dropped:"), 1)
        self.assertTrue(r.stderr.strip().endswith("sealed-window rows dropped: 3"))


class FrameAndTraceTests(unittest.TestCase):
    def test_frame_header_time_and_payload(self):
        f1 = frame(IN_NS)
        f2 = frame(OUT_NS, b"<t>2026-11-21T12:00:00Z</t>")
        f3 = frame(OUT_NS, jms_ms=IN_NS // 1_000_000)
        f4 = frame(OUT_NS, b"fine")
        frames, rest = sg.oaf1_frames(f1 + f2 + f3 + f4 + b"xx")
        self.assertEqual(rest, 2)
        self.assertEqual([sg.oaf1_frame_in_window(r, j, p) for _, r, j, p in frames], [True, True, True, False])

    def test_trace_rows_are_judged_by_time(self):
        ok = json.dumps({"timestamp": OUT_NS // NS, "trace": [[0, 1, 2], [5.5, 1, 2]]}).encode()
        bad = json.dumps({"timestamp": OUT_NS // NS, "trace": [[0, 1, 2], [(IN_NS - OUT_NS) / NS, 1, 2]]}).encode()
        odd = json.dumps({"timestamp": OUT_NS // NS, "trace": [["x", 1, 2]]}).encode()
        self.assertFalse(sg.adsb_trace_in_window(ok))
        self.assertTrue(sg.adsb_trace_in_window(bad))
        self.assertTrue(sg.adsb_trace_in_window(odd))


class FilterExistingTests(unittest.TestCase):
    def test_filter_existing_deletes_matches_and_prints_only_counts(self):
        with tempfile.TemporaryDirectory() as t:
            corpus = Path(t) / "corpus"
            (corpus / "sub").mkdir(parents=True)
            (corpus / "a-bad").write_bytes(b"<x>2026-11-21T12:00:00Z</x>")
            (corpus / "sub" / "b-ok").write_bytes(b"<x>2026-11-10T12:00:00Z</x>")
            (corpus / "c-frame").write_bytes(frame(OUT_NS) + frame(IN_NS))
            (corpus / "d-trace").write_bytes(json.dumps({"timestamp": 0, "trace": [[(IN_NS) / NS, 1]]}).encode())
            (corpus / "syn-seed").write_bytes(b"2026-13-45T25:61:61Z")
            r = subprocess.run([sys.executable, "-I", str(REPO / "fuzz" / "make_corpora.py"), "--filter-existing", str(corpus)],
                               capture_output=True, text=True, env={"PATH": os.environ.get("PATH", "")})
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(r.stdout, "files scanned: 5\nfiles removed: 3\nfiles kept: 2\n")
            self.assertTrue(r.stderr.strip().endswith("sealed-window rows dropped: 3"))
            self.assertEqual(sorted(p.name for p in corpus.rglob("*") if p.is_file()), ["b-ok", "syn-seed"])
            for name in ("a-bad", "c-frame", "d-trace", "b-ok"):
                self.assertNotIn(name, r.stdout + r.stderr)


class CorpusBuilderTests(unittest.TestCase):
    """make_corpora.py run on a synthetic data root in a copy of the tree it needs."""

    def test_recorded_messages_frames_and_traces_in_the_window_are_not_copied(self):
        with tempfile.TemporaryDirectory() as t:
            tree = Path(t) / "repo"
            data = Path(t) / "data"
            (tree / "tools").mkdir(parents=True)
            shutil.copy(REPO / "tools" / "sealed_guard.py", tree / "tools")
            (tree / "crates" / "oa-sealed").mkdir(parents=True)
            lst = (REPO / "crates" / "oa-sealed" / "sealed_paths.txt").read_text()
            lst = "".join(f"root {data}\n" if l.startswith("root ") else l + "\n" for l in lst.split("\n") if l or False)
            (tree / "crates" / "oa-sealed" / "sealed_paths.txt").write_text(lst)
            shutil.copytree(REPO / "fuzz", tree / "fuzz", ignore=shutil.ignore_patterns("target", "fuzz_targets"))
            shutil.copytree(REPO / "crates" / "oa-arinc424" / "tests" / "fixtures", tree / "crates" / "oa-arinc424" / "tests" / "fixtures")
            (tree / "crates" / "oa-notam" / "tests" / "common").mkdir(parents=True)
            shutil.copy(REPO / "crates" / "oa-notam" / "tests" / "common" / "mod.rs", tree / "crates" / "oa-notam" / "tests" / "common")
            for f in ("cat048.raw", "cat062cat065.raw", "cat21_re.ast"):
                (data / "raw" / "asterix-samples").mkdir(parents=True, exist_ok=True)
                (data / "raw" / "asterix-samples" / f).write_bytes(b"\x30\x00\x05ab")
            nasr = data / "raw" / "nasr-2026-10-29" / "x"
            nasr.mkdir(parents=True)
            for kind in ("STAR", "DP", "AWY"):
                for part in ("BASE", "APT", "RTE", "SEG_ALT"):
                    (nasr / f"{kind}_{part}.csv").write_text('"A","B"\n"1","2"\n')
            (data / "scds" / "tbfm-a").mkdir(parents=True)
            (data / "scds" / "tbfm-a" / "ok.msg").write_bytes(b"<m t='2026-11-10T12:00:00Z'/>")
            (data / "scds" / "tbfm-a" / "bad.msg").write_bytes(b"<m t='2026&#45;11&#45;21T12:00:00Z'/>")
            (data / "scds" / "rec").mkdir()
            (data / "scds" / "rec" / "x.oaf").write_bytes(frame(IN_NS, b"in") + frame(OUT_NS, b"good") + frame(OUT_NS, b"<t>2026-11-22T01:00:00Z</t>"))
            (data / "raw" / "adsblol" / "2026.11.10").mkdir(parents=True)
            with tarfile.open(data / "raw" / "adsblol" / "2026.11.10" / "v1.tar.aa", "w") as tf:
                for name, tr in (("ok.json", OUT_NS // NS), ("bad.json", IN_NS // NS)):
                    raw = gzip.compress(json.dumps({"timestamp": tr, "trace": [[0, 1, 2]]}).encode())
                    ti = tarfile.TarInfo(name)
                    ti.size = len(raw)
                    tf.addfile(ti, io.BytesIO(raw))
            r = subprocess.run([sys.executable, "-I", str(tree / "fuzz" / "make_corpora.py")], capture_output=True, text=True,
                               env={"PATH": os.environ.get("PATH", ""), "OPENATCA_DATA": str(data)})
            self.assertEqual(r.returncode, 0, r.stderr + r.stdout)
            c = data / "fuzz"
            self.assertEqual(sorted(p.name for p in (c / "tbfm" / "corpus").iterdir() if not p.name.startswith(("syn-", "seed-"))), ["ok.msg"])
            frames = sorted((c / "feed-frame" / "corpus").iterdir())
            real = [p for p in frames if not p.name.startswith(("syn-", "seed-"))]
            self.assertEqual([p.read_bytes() for p in real], [frame(OUT_NS, b"good")])
            traces = [p for p in (c / "adsb-trace" / "corpus").iterdir() if not p.name.startswith(("syn-", "seed-"))]
            self.assertEqual(len(traces), 1)
            self.assertEqual(json.loads(traces[0].read_bytes())["timestamp"], OUT_NS // NS)
            # one xml message, two frames, one joined run, one trace
            self.assertTrue(r.stderr.strip().endswith("sealed-window rows dropped: 5"), r.stderr)


def write_rows(path, fields, rows):
    with open(path, "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fields, lineterminator="\n")
        w.writeheader()
        w.writerows(rows)


class ConsumerTests(unittest.TestCase):
    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.d = Path(self._t.name)

    def tearDown(self):
        self._t.cleanup()

    def test_evaluator_drops_in_window_and_unparsable_rows(self):
        rows = [dict(scenario_id="s", flight_id="f", time_ns=v, lat_deg=0, lon_deg=0, ve_mps=0, vn_mps=0, phase="p")
                for v in (OUT_NS, OUT_NS + NS, IN_NS)]
        write_rows(self.d / "t.csv", ce.EXPECTED["truth"], rows)
        before = ce.DROPS.count
        self.assertEqual(len(ce._csv(str(self.d / "t.csv"), "truth")), 2)
        self.assertEqual(ce.DROPS.count, before + 1)
        out = [dict(scenario_id="s", flight_id="f", score_ns=OUT_NS, source_ns=IN_NS, publish_ns=OUT_NS, lat_deg=0,
                    lon_deg=0, ve_mps=0, vn_mps=0, status="valid"),
               dict(scenario_id="s", flight_id="f", score_ns=OUT_NS + NS, source_ns="", publish_ns="", lat_deg="",
                    lon_deg="", ve_mps="", vn_mps="", status="missing")]
        write_rows(self.d / "o.csv", ce.EXPECTED["output"], out)
        self.assertEqual(len(ce._csv(str(self.d / "o.csv"), "output")), 1)

    def test_baseline_output_skips_in_window_grid_rows(self):
        write_rows(self.d / "g.csv", ce.EXPECTED["grid"], [dict(scenario_id="s", flight_id="f", time_ns=v) for v in (OUT_NS, IN_NS)])
        write_rows(self.d / "m.csv", ce.EXPECTED["measurements"], [])
        m = cb.run(None, str(self.d / "m.csv"), str(self.d / "o.csv"), grid_path=str(self.d / "g.csv"))
        self.assertEqual(m["output_rows"], 1)

    def test_track_set_score_drops_in_window_rows(self):
        rows = [dict(scenario_id="s", time_ns=v, truth_id="a", lat_deg=0, lon_deg=0) for v in (OUT_NS, IN_NS, "zz")]
        write_rows(self.d / "t.csv", tss.TRUTH_FIELDS, rows)
        before = tss.DROPS.count
        pts, _ = tss._bounded_csv(self.d / "t.csv", tss.TRUTH_FIELDS, "truth")
        self.assertEqual(len(pts), 1)
        self.assertEqual(tss.DROPS.count, before + 2)

    def test_reference_drops_in_window_rows(self):
        rows = [dict(scenario_id="s", flight_id=1, time_ns=v, lat_deg=0, lon_deg=0, ve_mps=0, vn_mps=0, phase="p")
                for v in (OUT_NS, IN_NS)]
        write_rows(self.d / "t.csv", ref.TRUTH_FIELDS, rows)
        before = ref.DROPS.count
        self.assertEqual(len(list(ref._bounded_rows(self.d / "t.csv", ref.TRUTH_FIELDS, 10))), 1)
        self.assertEqual(ref.DROPS.count, before + 1)

    def test_run_comparison_key_reader_drops_in_window_rows(self):
        write_rows(self.d / "g.csv", ["scenario_id", "flight_id", "time_ns"],
                   [dict(scenario_id="s", flight_id="f", time_ns=v) for v in (OUT_NS, IN_NS)])
        before = rc.DROPS.count
        self.assertEqual(len(rc._csv_keys(self.d / "g.csv", ["scenario_id", "flight_id", "time_ns"])), 1)
        self.assertEqual(rc.DROPS.count, before + 1)

    def test_motion_study_drops_rows_and_refuses_sealed_paths(self):
        rows = [dict(scenario_id="s", flight_id=1, time_ns=v, lat_deg=0, lon_deg=0, ve_mps=0, vn_mps=0, phase="p")
                for v in (OUT_NS, IN_NS)]
        write_rows(self.d / "t.csv", ms.TRUTH_FIELDS, rows)
        before = ms.DROPS.count
        self.assertEqual(len(ms._csv(self.d / "t.csv", ms.TRUTH_FIELDS)), 1)
        self.assertEqual(ms.DROPS.count, before + 1)
        sealed_dir = self.d / "scds" / "stdds-20261119-holdout"
        sealed_dir.mkdir(parents=True)
        write_rows(sealed_dir / "truth.csv", ms.TRUTH_FIELDS, [])
        for call in (lambda: ms._csv(sealed_dir / "truth.csv", ms.TRUTH_FIELDS), lambda: ms.sha256(sealed_dir / "truth.csv"),
                     lambda: ms.read_json(sealed_dir / "x.json"), lambda: ms._load_run(sealed_dir, self.d / "b.json")):
            with self.assertRaises(ms.StudyError) as c:
                call()
            self.assertIn("OA-SEAL-001", str(c.exception))
        self.assertEqual(ms.main(["score", "--candidate-dir", str(sealed_dir), "--candidate-build", "b", "--default-dir", "d",
                                  "--default-build", "b", "--out", str(self.d / "o.json")]), 2)


if __name__ == "__main__":
    unittest.main()
