"""Synthetic conformance examples for the machine-readable format references."""

import hashlib
import json
import struct
import unittest
from pathlib import Path

import reference_readers as readers


def oag2adpt(version: int, snapshot: bytes, preset: bytes | None = None) -> bytes:
    release, facility, change = b"rel-1", b"ZAU", b"CHG-2"
    issue_text = (b"issuer-1", b"approver-2", b"Synthetic issue")
    issue_header = b""
    tail_text = b""
    if version >= 2:
        issue_header = (
            struct.pack("<Q", 2)
            + b"p" * 32
            + b"\x02"
            + bytes((len(issue_text[0]), len(issue_text[1])))
            + struct.pack("<H", len(issue_text[2]))
        )
        tail_text = b"".join(issue_text)
    preset_header = b""
    if version == 3:
        assert preset is not None
        preset_header = struct.pack("<Q", len(preset)) + hashlib.sha256(preset).digest()
    header = (
        b"OAG2ADPT"
        + struct.pack("<HHH", version, 1, 1)
        + bytes((len(release), len(facility), len(change), 2 if version == 3 else 1))
        + struct.pack("<Q", len(snapshot))
        + hashlib.sha256(snapshot).digest()
    )
    return header + issue_header + preset_header + release + facility + change + tail_text


class FormatSpecTests(unittest.TestCase):
    def test_machine_readable_specs_are_json_and_reference_existing_public_files(self):
        filenames = (
            "iwxxm-2025-2-metar-speci-profile.json",
            "oag2adpt-v1-v3.json",
            "oa-regional-j2-formats-v1.json",
        )
        for filename in filenames:
            with (Path(__file__).parent / filename).open(encoding="utf-8") as stream:
                self.assertIsInstance(json.load(stream), dict)
        iwxxm = readers.load_spec(filenames[0])
        self.assertEqual(iwxxm["standard"]["edition"], "2025-2")
        self.assertEqual(len(iwxxm["implementation_and_tests"]["official_examples"]), 8)
        regional = readers.load_spec(filenames[2])
        self.assertEqual(regional["operated_replay"]["settings"]["header"], "OA_OPERATED_SETTINGS\t1")
        self.assertEqual(regional["operated_replay"]["manifest"]["header"], "OA_OPERATED_MANIFEST\t1")
        for record in iwxxm["standard"]["schema_files"]:
            schema = (Path(__file__).parent / record["path"]).resolve().read_bytes()
            self.assertEqual(hashlib.sha256(schema).hexdigest(), record["sha256"])

    def test_oag2adpt_reader_decodes_all_supported_manifest_versions(self):
        snapshot = b"synthetic snapshot bytes"
        preset = b"synthetic ca preset"
        for version in (1, 2, 3):
            with self.subTest(version=version):
                manifest = oag2adpt(version, snapshot, preset if version == 3 else None)
                decoded = readers.read_oag2adpt(
                    manifest,
                    snapshot,
                    ca_preset=preset if version == 3 else None,
                    expected_manifest_sha256=hashlib.sha256(manifest).hexdigest(),
                )
                self.assertEqual(decoded["schema_version"], version)
                self.assertEqual(decoded["snapshot_sha256"], hashlib.sha256(snapshot).hexdigest())
                self.assertEqual(decoded["release"], "rel-1")
                self.assertEqual(decoded["facility"], "ZAU")
                if version >= 2:
                    self.assertEqual(decoded["change_class"], 2)
                if version == 3:
                    self.assertEqual(decoded["ca_preset_sha256"], hashlib.sha256(preset).hexdigest())

    def test_oag2adpt_reader_refuses_unbound_noncanonical_and_bad_component_rows(self):
        snapshot = b"synthetic snapshot"
        version_one = oag2adpt(1, snapshot)
        with self.assertRaises(readers.FormatError):
            readers.read_oag2adpt(version_one + b"x", snapshot)
        wrong_component_count = bytearray(version_one)
        wrong_component_count[17] = 2
        with self.assertRaises(readers.FormatError):
            readers.read_oag2adpt(bytes(wrong_component_count), snapshot)
        with self.assertRaises(readers.FormatError):
            readers.read_oag2adpt(version_one, b"other snapshot")
        with self.assertRaises(readers.FormatError):
            readers.read_oag2adpt(version_one, snapshot, expected_manifest_sha256="0" * 64)

    def test_regional_settings_reader_and_recorded_profile_rows(self):
        settings = (
            "OA_REGIONAL_SETTINGS\t1\n"
            "centre\tZAU1\n"
            "profile\tRECORDED_PEAK_RENDERED_V1\n"
            "tracks\t4096\nflights\t4096\nframes\t512\n"
            "surveillance_max_frames\t20000000\nmax_total_events\t10000000\n"
            "max_drain_rows\t262144\nmax_baro_evidence\t1000000\n"
            "max_open_alerts\t1048576\nmax_candidate_pairs\t8388608\n"
            "max_ca_work_per_frame\t8388608\nmax_flight_log_entries\t1024\n"
            "max_presentation_updates\t8192\nrender_scope\ttrue\n"
            "configuration\tSurveillanceConfig { centre_lat_e7: 410000000, centre_lon_e7: -870000000, range_nm: 180.0 }\n"
        ).encode()
        values = readers.read_settings(settings)
        self.assertEqual(values["profile"], "RECORDED_PEAK_RENDERED_V1")
        self.assertEqual(values["configuration"].split(" {")[0], "SurveillanceConfig")
        self.assertEqual(readers.read_regional_profile(settings), ("RECORDED_PEAK_RENDERED_V1", "recorded-peak-rendered"))
        with self.assertRaises(readers.FormatError):
            readers.read_regional_profile(settings.replace(b"tracks\t4096", b"tracks\t4095"))
        self.assertEqual(
            readers.read_regional_profile(b"OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\n"),
            ("DEFAULT_V1", None),
        )
        with self.assertRaises(readers.FormatError):
            readers.read_settings(settings + b"tracks\t1\n")

    def test_regional_run_manifest_reader(self):
        digest = "a" * 64
        manifest = (
            "OA_REGIONAL_MANIFEST\t1\n"
            f"input_sha256\t{digest}\nsnapshot_sha256\t{digest}\n"
            f"executable_sha256\t{digest}\nsettings_sha256\t{digest}\n"
            f"frame\t1\t123456789\tframe-0001.txt\t{digest}\t{digest}\n"
        ).encode()
        fields, frames = readers.read_regional_manifest(manifest)
        self.assertEqual(fields["input_sha256"], digest)
        self.assertEqual(frames[0]["evidence"], "frame-0001.txt")
        with self.assertRaises(readers.FormatError):
            readers.read_regional_manifest(manifest.replace(b"frame-0001.txt", b"wrong.txt"))

    def test_recording_import_manifest_reader(self):
        digest = "b" * 64
        row_values = {
            "centre": "ZAU1", "normalized_bytes": "1", "normalized_sha256": digest,
            "provenance_bytes": "2", "provenance_sha256": digest,
            "quarantine_bytes": "3", "quarantine_sha256": digest,
            "report_bytes": "4", "report_sha256": digest, "executable_sha256": digest,
            "physical_input_cap_bytes": "16777216", "expanded_input_cap_bytes": "16777216",
            "frame_cap": "100000", "stream_cap": "8", "normalized_input_cap_bytes": "16777216",
            "normalized_event_cap": "100000", "events_per_receipt_cap": "256", "runtime_group_cap": "512",
            "runtime_gap_cap_ns": "60000000000", "quarantine_record_cap": "462144", "quarantine_bytes_cap": "67108864",
        }
        text = "OA_RECORDING_IMPORT_MANIFEST\t1\n" + "".join(f"{key}\t{value}\n" for key, value in row_values.items())
        text += f"input_stream\t0\tADSB\t{digest}\t5\t5\t0\n"
        fields, streams = readers.read_recording_manifest(text.encode())
        self.assertEqual(fields["centre"], "ZAU1")
        self.assertEqual(streams[0]["source"], "ADSB")
        with self.assertRaises(readers.FormatError):
            readers.read_recording_manifest(text.replace("stream_cap\t8", "stream_cap\t8\nstream_cap\t8").encode())

    def test_portability_json_reader_binds_manifest_and_inventory(self):
        payloads = {"recording/normalized.tsv": b"OA-RS\t1\nv1\n"}
        document = {
            "schema": "openatca-j2-regional-portability-v1",
            "source_commit": "c" * 40,
            "recording_index_sha256": "d" * 64,
            "baseline_manifest_sha256": "e" * 64,
            "files": {name: {"bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()}
                      for name, content in payloads.items()},
        }
        manifest = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
        parsed = readers.read_portability(manifest, (readers.sha256(manifest) + "\n").encode(), payloads)
        self.assertEqual(parsed["source_commit"], "c" * 40)
        with self.assertRaises(readers.FormatError):
            readers.read_portability(manifest, b"0" * 64 + b"\n", payloads)

    def test_rust_pretty_debug_frame_reader_handles_nested_values_and_escapes(self):
        values = [
            "sequence: 1", "input: FrameEvidence { receipt_ns: 7, input_events: 1 }",
            'surveillance: SurveillanceFrame { warnings: ["comma, brace }", "line\\nnext"] }',
            "flight_operations: []", "flights: []", "flight_report: FlightDriverReport { flights: 0 }",
            "refused_legacy_route_events: 0", "controller_views: []", "controller_dispatches: []",
            "controller_outcome: Applied", "controller_selection_kind: None",
            "controller_selected_address: None", "controller_selection: None",
            'console_text: "quote \\" and comma,\\nline"', "quarantine: RuntimeQuarantineReport { records: [] }",
        ]
        payload = "RegionalFrame {\n" + "\n".join(f"    {item}," for item in values) + "\n}\n"
        record = ("OA_REGIONAL_FRAME\t1\n" + payload).encode()
        decoded = readers.read_regional_frame(record)
        self.assertIn("comma, brace", decoded["fields"]["surveillance"])
        self.assertEqual(tuple(decoded["fields"]), readers.REGIONAL_FRAME_FIELDS)

    def test_iwxxm_synthetic_root_envelope_and_refusal(self):
        namespace = "http://icao.int/iwxxm/2025-2"
        gml = "http://www.opengis.net/gml/3.2"
        valid = (
            f'<iwxxm:METAR xmlns:iwxxm="{namespace}" xmlns:gml="{gml}" '
            'gml:id="synthetic-1" reportStatus="NORMAL" permissibleUsage="OPERATIONAL">'
            '<iwxxm:issueTime/><iwxxm:aerodrome/><iwxxm:observationTime/><iwxxm:observation/>'
            '</iwxxm:METAR>'
        ).encode()
        decoded = readers.read_iwxxm_envelope(valid)
        self.assertEqual(decoded["kind"], "METAR")
        self.assertEqual(decoded["gml_id"], "synthetic-1")
        with self.assertRaises(readers.FormatError):
            readers.read_iwxxm_envelope(valid.replace(b"<iwxxm:observationTime/>", b""))

    def test_operated_manifest_reader(self):
        digest = "c" * 64
        manifest = (
            "OA_OPERATED_MANIFEST\t1\nmode\tRECORDED_REPLAY\n"
            f"input_sha256\t{digest}\nmodel_sha256\t{digest}\n"
            f"executable_sha256\t{digest}\nsettings\tsettings.tsv\t{digest}\n"
            f"operator_journal\toperator.tsv\t{digest}\nscript_sha256\t{digest}\n"
            f"frame\t1\t123456789\tframe-0001.txt\t{digest}\tframe-0001.ppm\t{digest}\n"
        ).encode()
        fields, frames = readers.read_operated_manifest(manifest)
        self.assertEqual(fields["mode"], "RECORDED_REPLAY")
        self.assertEqual(frames[0]["ppm_filename"], "frame-0001.ppm")
        with self.assertRaises(readers.FormatError):
            readers.read_operated_manifest(manifest.replace(b"frame-0001.ppm", b"elsewhere.ppm"))

    def test_operated_debug_frame_hash_prefix(self):
        digest = "f" * 64
        values = [
            "sequence: 1", "input: FrameEvidence {}", "surveillance: SurveillanceFrame {}",
            "flight_operations: []", "flights: []", "flight_report: FlightDriverReport {}",
            "refused_legacy_route_events: 0", "controller_views: []", "controller_dispatches: []",
            "controller_outcome: Applied", "controller_selection_kind: None",
            "controller_selected_address: None", "controller_selection: None", "console_text: \"\"",
            "quarantine: RuntimeQuarantineReport {}",
        ]
        debug = "RegionalFrame {\n" + "\n".join(f"    {field}," for field in values) + "\n}\n"
        record = (f"OA_REGIONAL_OPERATED_FRAME\t1\npixels_ppm_sha256\t{digest}\n"
                  f"rendered_frame_sha256\t{digest}\n{debug}").encode()
        self.assertEqual(readers.read_regional_frame(record, operated=True)["hashes"]["pixels_ppm_sha256"], digest)


if __name__ == "__main__":
    unittest.main()
