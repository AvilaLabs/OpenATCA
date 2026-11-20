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
import zlib
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


def frame(recv_ns, payload=b"hello", jms_ms=0, seq=1, kind=1, crc=True):
    body = struct.pack("<4sBBHQqqI", b"OAF1", kind, 0, 0, seq, recv_ns, jms_ms, len(payload)) + payload
    return body + struct.pack("<I", zlib.crc32(body) if crc else zlib.crc32(body) ^ 1)


def frame2(recv_ns, payload=b"hello", jms_ms=0, seq=1, kind=1, crc=True, mono=5):
    body = struct.pack("<4sBBHQqqQI", b"OAF2", kind, 0, 0, seq, recv_ns, jms_ms, mono, len(payload)) + payload
    return body + struct.pack("<I", zlib.crc32(body) if crc else zlib.crc32(body) ^ 1)


def make_tree(base, git=False):
    """A copy of what make_corpora.py needs, with the list's root moved to a temp data root."""
    tree, data = Path(base) / "repo", Path(base) / "data"
    (tree / "tools").mkdir(parents=True)
    shutil.copy(REPO / "tools" / "sealed_guard.py", tree / "tools")
    (tree / "crates" / "oa-sealed").mkdir(parents=True)
    lst = (REPO / "crates" / "oa-sealed" / "sealed_paths.txt").read_text()
    lst = "".join(f"root {data}\n" if l.startswith("root ") else l + "\n" for l in lst.split("\n") if l)
    (tree / "crates" / "oa-sealed" / "sealed_paths.txt").write_text(lst)
    shutil.copytree(REPO / "fuzz", tree / "fuzz", ignore=shutil.ignore_patterns("target", "fuzz_targets"))
    shutil.copytree(REPO / "crates" / "oa-arinc424" / "tests" / "fixtures", tree / "crates" / "oa-arinc424" / "tests" / "fixtures")
    (tree / "crates" / "oa-notam" / "tests" / "common").mkdir(parents=True)
    shutil.copy(REPO / "crates" / "oa-notam" / "tests" / "common" / "mod.rs", tree / "crates" / "oa-notam" / "tests" / "common")
    data.mkdir()
    if git:
        env = {"PATH": os.environ.get("PATH", ""), "HOME": str(base)}
        for args in (["init", "-q"], ["add", "-A"], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x"]):
            subprocess.run(["git", "-C", str(tree), *args], check=True, capture_output=True, env=env)
    return tree, data


def run_filter(tree, directory):
    return subprocess.run([sys.executable, "-I", str(tree / "fuzz" / "make_corpora.py"), "--filter-existing", str(directory)],
                          capture_output=True, text=True, env={"PATH": os.environ.get("PATH", ""), "HOME": str(tree.parent)})


class TextScanTests(unittest.TestCase):
    # REQ: OA-SEAL-011
    def test_iso_forms_in_the_window_are_found(self):
        for t in ("2026-11-21T12:00:00Z", "2026-11-21T12:00:00.123456Z", "2026-11-21T12:00:00", "2026-11-21 12:00:00",
                  "2026-11-21T12:00Z", "20261121T120000Z", "20261121T1200Z", "2026-11-21T12:00:00+05:00",
                  "2026-11-21T12:00:00-0800", "2026-11-21", "20261121", "2026-11-23T00:20:00Z",
                  "2026-11-19T23:50:00Z", "202611211200", "20261121120000", "2026-11-21T24:00:00Z"):
            self.assertTrue(sealed(f"<a t='{t}'/>"), t)

    def test_times_outside_the_window_pass(self):
        for t in ("2026-11-10T12:00:00Z", "2026-11-18T23:30:00Z", "2026-11-23T00:40:00Z", "2026-12-01", "20261201T000000Z",
                  "2025-11-21T12:00:00Z", "no time here", "2026-11-18T12:00:00Z"):
            self.assertFalse(sealed(f"<a t='{t}'/>"), t)

    def test_an_offset_moves_the_instant(self):
        # window is [2026-11-19T23:45Z, 2026-11-23T00:32Z)
        self.assertTrue(sealed("2026-11-23T05:00:00+06:00"))   # 2026-11-22T23:00Z
        self.assertFalse(sealed("2026-11-23T05:40:00+05:00"))
        self.assertFalse(sealed("2026-11-19T22:00:00+01:00"))  # 21:00Z the day before
        self.assertTrue(sealed("2026-11-19T22:00:00-03:00"))   # 01:00Z on the 20th

    def test_a_time_without_a_zone_covers_utc_minus_14_to_plus_12(self):
        # the window ends 2026-11-23T00:32Z; local 13:00 and 14:00 on the 23rd are UTC 23:00 and 00:00
        self.assertTrue(sealed("2026-11-23T13:00:00"))
        self.assertTrue(sealed("2026-11-23T14:00:00"))
        self.assertFalse(sealed("2026-11-23T15:00:00"))
        # the window starts 2026-11-19T23:45Z; local 2026-11-19T11:00 is UTC 23:00 (+12h)... still before
        self.assertTrue(sealed("2026-11-19T12:00:00"))
        self.assertFalse(sealed("2026-11-19T11:00:00"))

    # REQ: OA-SEAL-011
    def test_forms_the_first_scan_missed(self):
        s = IN_NS // NS
        slips = [
            "98762026-11-20T00:00:00Z", "2026-11-20123", "20/11/2026", "11/20/2026", "20.11.2026", "2026/11/21", "2026.11.21",
            "20 Nov 2026", "20NOV26", "20th November 2026", "Nov 21, 2026", "november 2026", "26-11-20", "20-11-26", "11-20-26",
            "2026-325", "2026325", "2026-W47-5", "2026W475", "2026-W47", "20261121120000123", "202611211200001",
            "2026-11-<!-- c -->21", "<a>2026-11-</a><b>21</b>", "2026-11-2\n1T12:00:00Z", "2026 11 21",
            f"{s:,}", f"{s:_}", " ".join([f"{s}"[:1], f"{s}"[1:4], f"{s}"[4:7], f"{s}"[7:]]), f"{s * 1000:,}",
            format(s, ".6e"), format(s * 1000, ".5E"), f"{s}000000", f"{s * 10}", f"{s * 100}", f"{s * 10_000_000}",
            f"{s * 1_000_000}", f"x{s}x",
        ]
        for t in slips:
            self.assertTrue(sealed(f"<a>{t}</a>"), t)
        for t in ["20/11/2025", "20 Nov 2025", "2026-W30-1", "2026-100", "2025-11-21", f"{OUT_NS // NS:,}", format(OUT_NS // NS, ".6e"),
                  "2026-11-18T23:30:00Z", "version 1.2.3", "id 123456789012"]:
            self.assertFalse(sealed(f"<a>{t}</a>"), t)

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
        # a scoring reader keeps a cell that does not parse (its own validation rejects it, as on main)
        self.assertFalse(sg.time_in_window("12x", keep_malformed=True))
        for spelled in ("+" + str(IN_NS), "1_0", " " + str(IN_NS), str(IN_NS) + " ", "٣"):
            self.assertFalse(sg.time_in_window(spelled, keep_malformed=True), spelled)
        # ... except a reader that itself reads text with int() (motion_study): every spelling it accepts is judged
        self.assertTrue(sg.time_in_window(" " + str(IN_NS), keep_malformed=True, loose=True))
        self.assertTrue(sg.time_in_window("+" + str(IN_NS), keep_malformed=True, loose=True))
        self.assertTrue(sg.row_in_window({"a": str(IN_NS)}, ("a",), keep_malformed=True))
        self.assertFalse(sg.row_in_window({"a": "x"}, ("a", "b"), keep_malformed=True))

    def test_drop_counter_line_is_printed_once_at_exit_even_after_an_error(self):
        code = ("import sys; sys.path.insert(0, %r); import sealed_guard as g; c = g.DropCounter(); c.add(); c.add(2); "
                "raise SystemExit(3)" % str(Path(__file__).resolve().parent))
        r = subprocess.run([sys.executable, "-I", "-c", code], capture_output=True, text=True)
        self.assertEqual(r.returncode, 3)
        self.assertEqual(r.stderr.count("sealed-window rows dropped:"), 1)
        self.assertTrue(r.stderr.strip().endswith("sealed-window rows dropped: 3"))


class FrameAndTraceTests(unittest.TestCase):
    def test_frame_header_time_and_payload_for_oaf1_and_oaf2(self):
        for make in (frame, frame2):
            f1 = make(IN_NS)
            f2 = make(OUT_NS, b"<t>2026-11-21T12:00:00Z</t>")
            f3 = make(OUT_NS, jms_ms=IN_NS // 1_000_000)
            f4 = make(OUT_NS, b"fine")
            frames, rest = sg.oaf_frames(f1 + f2 + f3 + f4 + b"xx")
            self.assertEqual(rest, 2)
            self.assertEqual([sg.oaf_frame_dropped(f) for f in frames], [True, True, True, False])
        mixed, rest = sg.oaf_frames(frame(OUT_NS, b"a") + frame2(OUT_NS, b"bb", mono=9) + frame(OUT_NS, b"c"))
        self.assertEqual((rest, [f.payload for f in mixed]), (0, [b"a", b"bb", b"c"]))

    def test_oaf2_is_read_by_the_corpus_file_test(self):
        self.assertTrue(sg.file_has_sealed_time(frame2(OUT_NS, b"x") + frame2(IN_NS, b"y")))
        self.assertFalse(sg.file_has_sealed_time(frame2(OUT_NS, b"x") + frame2(OUT_NS + NS, b"y")))

    def test_bad_headers_are_dropped(self):
        for bad in (frame(0), frame(-5), frame(OUT_NS, jms_ms=-1), frame(OUT_NS, kind=9), frame(OUT_NS, kind=0),
                    frame(OUT_NS, crc=False), frame2(0), frame2(OUT_NS, kind=4), frame2(OUT_NS, crc=False)):
            (f,), _ = sg.oaf_frames(bad)
            self.assertTrue(sg.oaf_frame_dropped(f), bad)
        for good in (frame(OUT_NS, kind=2), frame2(OUT_NS, kind=3, jms_ms=OUT_NS // 1_000_000)):
            (f,), _ = sg.oaf_frames(good)
            self.assertFalse(sg.oaf_frame_dropped(f))

    def test_trace_rows_are_judged_by_time(self):
        ok = json.dumps({"timestamp": OUT_NS // NS, "trace": [[0, 1, 2], [5.5, 1, 2]]}).encode()
        bad = json.dumps({"timestamp": OUT_NS // NS, "trace": [[0, 1, 2], [(IN_NS - OUT_NS) / NS, 1, 2]]}).encode()
        odd = json.dumps({"timestamp": OUT_NS // NS, "trace": [["x", 1, 2]]}).encode()
        self.assertFalse(sg.adsb_trace_in_window(ok))
        self.assertTrue(sg.adsb_trace_in_window(bad))
        self.assertTrue(sg.adsb_trace_in_window(odd))


class FilterExistingTests(unittest.TestCase):
    def test_filter_existing_deletes_matches_whatever_the_name_and_prints_only_counts(self):
        with tempfile.TemporaryDirectory() as t:
            tree, data = make_tree(t, git=True)
            corpus = data / "fuzz" / "x" / "corpus"
            (corpus / "sub").mkdir(parents=True)
            (corpus / "a-bad").write_bytes(b"<x>2026-11-21T12:00:00Z</x>")
            (corpus / "sub" / "b-ok").write_bytes(b"<x>2026-11-10T12:00:00Z</x>")
            (corpus / "c-frame").write_bytes(frame(OUT_NS) + frame(IN_NS))
            (corpus / "e-oaf2").write_bytes(frame2(OUT_NS) + frame2(IN_NS))
            (corpus / "d-trace").write_bytes(json.dumps({"timestamp": 0, "trace": [[IN_NS / NS, 1]]}).encode())
            seed = next(p for p in (tree / "fuzz" / "seeds").rglob("*") if p.is_file())
            (corpus / "syn-seed").write_bytes(seed.read_bytes())   # the bytes of a committed seed: kept
            (corpus / "syn-x").write_bytes(b"<x>2026-11-21T12:00:00Z</x>")   # named like a seed, is not one: removed
            r = run_filter(tree, corpus)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(r.stdout, "files scanned: 7\nfiles removed: 5\nfiles kept: 2\nfiles refused (kept): 0\n"
                                       "files skipped (not regular): 0\n")
            self.assertTrue(r.stderr.strip().endswith("sealed-window rows dropped: 5"))
            self.assertEqual(sorted(p.name for p in corpus.rglob("*") if p.is_file()), ["b-ok", "syn-seed"])
            for name in ("a-bad", "c-frame", "d-trace", "b-ok", "syn-x"):
                self.assertNotIn(name, r.stdout + r.stderr)

    def test_filter_existing_refuses_anything_that_is_not_a_corpus_directory(self):
        with tempfile.TemporaryDirectory() as t:
            tree, data = make_tree(t, git=True)
            keep = [data / "raw" / "2026.11.20" / "a.bin", data / "scds" / "stdds-20261119-holdout" / "b.bin",
                    data / "derived" / "c.bin", data / "fuzz" / "x" / "other" / "d.bin", data / "fuzz" / "x" / "corpus-not" / "e.bin"]
            for p in keep:
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(b"2026-11-21T12:00:00Z")
            for d in (data, data / "raw", data / "raw" / "2026.11.20", data / "scds", data / "scds" / "stdds-20261119-holdout",
                      data / "derived", data / "fuzz", data / "fuzz" / "x", data / "fuzz" / "x" / "other",
                      data / "fuzz" / "x" / "corpus-not", data / "nowhere", data / "fuzz" / "x" / "corpus"):
                r = run_filter(tree, d)
                self.assertNotEqual(r.returncode, 0, str(d))
                self.assertNotIn("files scanned", r.stdout)
            self.assertTrue(all(p.exists() for p in keep))

    def test_filter_existing_refuses_when_the_fuzz_tree_is_not_clean(self):
        with tempfile.TemporaryDirectory() as t:
            tree, data = make_tree(t, git=True)
            corpus = data / "fuzz" / "x" / "corpus"
            corpus.mkdir(parents=True)
            (corpus / "a").write_bytes(b"2026-11-21T12:00:00Z")
            (tree / "fuzz" / "untracked.bin").write_bytes(b"x")
            r = run_filter(tree, corpus)
            self.assertEqual(r.returncode, 2)
            self.assertTrue((corpus / "a").exists())
            (tree / "fuzz" / "untracked.bin").unlink()
            (tree / "fuzz" / "seal_times.dict").write_bytes(b"changed")
            self.assertEqual(run_filter(tree, corpus).returncode, 2)
            self.assertTrue((corpus / "a").exists())

    def test_filter_corpus_dir_prunes_sealed_directories_and_never_deletes_what_it_cannot_judge(self):
        with tempfile.TemporaryDirectory() as t:
            data = Path(t).resolve() / "data"
            corpus = data / "fuzz" / "x" / "corpus"
            sealed_sub = corpus / "scds" / "stdds-20261119-holdout"
            day_sub = corpus / "raw" / "2026.11.20"
            for d in (sealed_sub, day_sub):
                d.mkdir(parents=True)
                (d / "f").write_bytes(b"2026-11-21T12:00:00Z")
            (corpus / "ok").write_bytes(b"2026-11-10T12:00:00Z")
            (corpus / "bad").write_bytes(b"2026-11-21T12:00:00Z")
            os.mkfifo(corpus / "pipe")
            os.symlink(corpus / "ok", corpus / "link")
            (corpus / "linked").write_bytes(b"2026-11-21T12:00:00Z")
            os.link(corpus / "linked", Path(t) / "other-name")
            g = sg.Guard(root=data)
            counter = sg.DropCounter(announce=False)
            self.assertEqual(sg.filter_corpus_dir(corpus, counter, g), (5, 1, 4, 1, 2))
            self.assertEqual(counter.count, 1)
            self.assertFalse((corpus / "bad").exists())
            for kept in ((sealed_sub / "f"), (day_sub / "f"), corpus / "ok", corpus / "linked"):
                self.assertTrue(kept.exists(), kept)
            # not a directory, and not a corpus directory
            with self.assertRaises(NotADirectoryError):
                sg.filter_corpus_dir(corpus / "ok", counter, g)
            for d in (data, data / "fuzz", data / "fuzz" / "x", corpus / "scds" / "stdds-20261119-holdout"):
                with self.assertRaises(sg.SealedError):
                    sg.filter_corpus_dir(d, counter, g)


class MakeCorporaStaticTests(unittest.TestCase):
    def test_no_bare_read_listing_or_stat_outside_the_guard(self):
        import re
        src = (REPO / "fuzz" / "make_corpora.py").read_text()
        for i, line in enumerate(src.split("\n"), 1):
            code = line.split("#")[0]
            if re.search(r"(?<![\w.])open\(", code) and "'wb'" not in code:
                self.fail(f"make_corpora.py:{i}: bare open( for reading")
            for bad in ("os.listdir(", "glob.glob(", "os.path.getsize(", "os.path.exists(", "os.path.isdir(", "os.scandir(",
                        "os.walk(", "os.stat(", "Path(", ".read_bytes(", ".read_text(", "import glob", "shutil.", "io.open(",
                        "os.path.lexists("):
                self.assertNotIn(bad, code, f"make_corpora.py:{i}")
            for writer in ("tarfile.open(", "zipfile.ZipFile("):
                if writer in code:
                    self.assertTrue("fileobj=" in code or "(bio" in code, f"make_corpora.py:{i}: {writer} on a path")

    def test_guard_glob_and_listdir_never_return_a_sealed_name(self):
        with tempfile.TemporaryDirectory() as t:
            d = Path(t) / "scds"
            for n in ("tbfm-a", "stdds-20261119-holdout", "rec-20261119"):
                (d / n).mkdir(parents=True)
                (d / n / "a.msg").write_bytes(b"x")
            (d / "rec-20261119" / "day2").mkdir()
            self.assertEqual(sg.glob(str(d / "*" / "*.msg")), [str(d / "rec-20261119" / "a.msg"), str(d / "tbfm-a" / "a.msg")])
            self.assertEqual(sg.listdir(d), ["rec-20261119", "tbfm-a"])
            with self.assertRaises(sg.SealedError):
                sg.getsize(d / "stdds-20261119-holdout" / "a.msg")
            with self.assertRaises(sg.SealedError):
                sg.listdir(d / "rec-20261119" / "day2")


class RecordingDateTests(unittest.TestCase):
    def test_the_latest_date_in_a_path(self):
        self.assertEqual(sg.path_date("tbfm-20261110/x.msg"), (2026, 11, 10))
        self.assertEqual(sg.path_date("day-2026-11-18/rec/20261112.oaf"), (2026, 11, 18))
        self.assertEqual(sg.path_date("a_2026_11_09/b"), (2026, 11, 9))
        self.assertIsNone(sg.path_date("rec/x.oaf"))
        self.assertIsNone(sg.path_date("tbfm-20261399/x.msg"))
        self.assertIsNone(sg.path_date("tbfm-12345/x.msg"))


class CorpusBuilderTests(unittest.TestCase):
    """make_corpora.py run on a synthetic data root in a copy of the tree it needs."""

    def test_recorded_messages_frames_and_traces_in_the_window_are_not_copied(self):
        with tempfile.TemporaryDirectory() as t:
            tree, data = make_tree(t)
            for f in ("cat048.raw", "cat062cat065.raw", "cat21_re.ast"):
                (data / "raw" / "asterix-samples").mkdir(parents=True, exist_ok=True)
                (data / "raw" / "asterix-samples" / f).write_bytes(b"\x30\x00\x05ab")
            nasr = data / "raw" / "nasr-2026-10-29" / "x"
            nasr.mkdir(parents=True)
            for kind in ("STAR", "DP", "AWY"):
                for part in ("BASE", "APT", "RTE", "SEG_ALT"):
                    (nasr / f"{kind}_{part}.csv").write_text('"A","B"\n"1","2"\n')
            for d, name, body in (("tbfm-20261110", "ok.msg", b"<m t='2026-11-10T12:00:00Z'/>"),
                                  ("tbfm-20261110", "bad.msg", b"<m t='2026&#45;11&#45;21T12:00:00Z'/>"),
                                  ("tbfm-20261119", "late.msg", b"<m/>"),     # dated after the last allowed day
                                  ("tbfm-undated", "u.msg", b"<m/>")):        # no date in the path
                (data / "scds" / d).mkdir(parents=True, exist_ok=True)
                (data / "scds" / d / name).write_bytes(body)
            (data / "scds" / "rec").mkdir()
            good1, good2 = frame(OUT_NS, b"good"), frame2(OUT_NS + NS, b"good2")
            (data / "scds" / "rec" / "20261110.oaf").write_bytes(
                frame(IN_NS, b"in") + good1 + frame(OUT_NS, b"<t>2026-11-22T01:00:00Z</t>") + frame(OUT_NS, b"crc", crc=False) + good2)
            (data / "scds" / "rec" / "20261119.oaf").write_bytes(frame(OUT_NS, b"later"))
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
            real = [p for p in (c / "feed-frame" / "corpus").iterdir() if not p.name.startswith(("syn-", "seed-"))]
            contents = [p.read_bytes() for p in real]
            self.assertIn(good1, contents)
            self.assertIn(good2, contents)
            for blob in contents:   # every copied file is whole good frames only
                frames, rest = sg.oaf_frames(blob)
                self.assertEqual(rest, 0)
                self.assertTrue(frames and not any(sg.oaf_frame_dropped(f) for f in frames))
            traces = [p for p in (c / "adsb-trace" / "corpus").iterdir() if not p.name.startswith(("syn-", "seed-"))]
            self.assertEqual(len(traces), 1)
            self.assertEqual(json.loads(traces[0].read_bytes())["timestamp"], OUT_NS // NS)
            # the count is of dropped rows: in-window xml 1, later-dated xml 1, undated xml 1, three bad
            # frames (in window, payload time, bad CRC; a run holding one is not counted again), the later-dated
            # recording 1, the in-window trace 1
            self.assertTrue(r.stderr.strip().endswith("sealed-window rows dropped: 8"), r.stderr)


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

    def test_evaluator_drops_in_window_rows_but_unparsable_ones_still_raise(self):
        rows = [dict(scenario_id="s", flight_id="f", time_ns=v, lat_deg=0, lon_deg=0, ve_mps=0, vn_mps=0, phase="p")
                for v in (OUT_NS, OUT_NS + NS, IN_NS)]
        write_rows(self.d / "t.csv", ce.EXPECTED["truth"], rows)
        before = ce.DROPS.count
        self.assertEqual(len(ce._csv(str(self.d / "t.csv"), "truth")), 2)
        self.assertEqual(ce.DROPS.count, before + 1)
        rows[1]["time_ns"] = "1_000_000_000"
        write_rows(self.d / "t2.csv", ce.EXPECTED["truth"], rows)
        with self.assertRaises(ce.EvaluationError):
            ce._csv(str(self.d / "t2.csv"), "truth")
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
        # the in-window row is dropped; the unparsable one stays for the scorer's own validation
        self.assertEqual([r["time_ns"] for r in pts], [str(OUT_NS), "zz"])
        self.assertEqual(tss.DROPS.count, before + 1)

    def test_reference_drops_in_window_rows(self):
        rows = [dict(scenario_id="s", flight_id=1, time_ns=v, lat_deg=0, lon_deg=0, ve_mps=0, vn_mps=0, phase="p")
                for v in (OUT_NS, IN_NS)]
        write_rows(self.d / "t.csv", ref.TRUTH_FIELDS, rows)
        before = ref.DROPS.count
        self.assertEqual(len(list(ref._bounded_rows(self.d / "t.csv", ref.TRUTH_FIELDS, 10))), 1)
        self.assertEqual(ref.DROPS.count, before + 1)
        rows[0]["time_ns"] = "1_0"
        write_rows(self.d / "t2.csv", ref.TRUTH_FIELDS, rows)
        with self.assertRaises(ref.ValidationError):
            ref._parse_truth(self.d / "t2.csv", 10)

    def test_run_comparison_key_reader_drops_in_window_rows(self):
        write_rows(self.d / "g.csv", ["scenario_id", "flight_id", "time_ns"],
                   [dict(scenario_id="s", flight_id="f", time_ns=v) for v in (OUT_NS, IN_NS, "bad")])
        before = rc.DROPS.count
        self.assertEqual(len(rc._csv_keys(self.d / "g.csv", ["scenario_id", "flight_id", "time_ns"])), 2)
        self.assertEqual(rc.DROPS.count, before + 1)

    def test_motion_study_drops_rows_and_refuses_sealed_paths(self):
        rows = [dict(scenario_id="s", flight_id=1, time_ns=v, lat_deg=0, lon_deg=0, ve_mps=0, vn_mps=0, phase="p")
                for v in (OUT_NS, IN_NS)]
        write_rows(self.d / "t.csv", ms.TRUTH_FIELDS, rows)
        before = ms.DROPS.count
        self.assertEqual(len(ms._csv(self.d / "t.csv", ms.TRUTH_FIELDS)), 1)
        self.assertEqual(ms.DROPS.count, before + 1)
        rows[0]["time_ns"] = " " + str(IN_NS)   # int() reads it, so it is judged and dropped
        write_rows(self.d / "t3.csv", ms.TRUTH_FIELDS, rows[:1])
        self.assertEqual(len(ms._csv(self.d / "t3.csv", ms.TRUTH_FIELDS)), 0)
        rows[0]["time_ns"] = "abc"
        write_rows(self.d / "t2.csv", ms.TRUTH_FIELDS, rows[:1])
        self.assertEqual(len(ms._csv(self.d / "t2.csv", ms.TRUTH_FIELDS)), 1)  # kept; the row checks reject it
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
