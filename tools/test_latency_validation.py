import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import check_latency_validation as validator

ROOT = Path(__file__).resolve().parents[1]
REPORT_PATH = ROOT / "validation" / "latency-v1.json"
SCRIPT = ROOT / "tools" / "check_latency_validation.py"


class LatencyValidationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline = validator.load_report(REPORT_PATH)

    def test_checked_in_aggregate_passes(self):
        validator.validate_report(self.baseline)
        result = subprocess.run([sys.executable, str(SCRIPT)], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)

    def assert_cli_rejects(self, report):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mutated.json"
            path.write_text(json.dumps(report, allow_nan=False), encoding="utf-8")
            result = subprocess.run([sys.executable, str(SCRIPT), str(path)], capture_output=True,
                                    text=True, check=False)
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_each_required_boolean_false_fails_cli(self):
        fields = [("checks", key) for key in validator.REQUIRED_CHECKS]
        fields += [("targets", key) for key in validator.REQUIRED_TARGETS]
        for section, key in fields:
            with self.subTest(section=section, key=key):
                altered = copy.deepcopy(self.baseline)
                altered[section][key] = False
                self.assert_cli_rejects(altered)

    def test_missing_and_nonboolean_target_claims_refuse(self):
        for value in (None, 1, "true"):
            with self.subTest(value=value):
                altered = copy.deepcopy(self.baseline)
                altered["checks"]["display_p99_under_100ms"] = value
                with self.assertRaises(validator.ValidationError):
                    validator.validate_report(altered)
        for section, key in (("checks", "alert_in_creating_scan"), ("targets", "alert_same_scan")):
            altered = copy.deepcopy(self.baseline)
            del altered[section][key]
            with self.subTest(missing=(section, key)), self.assertRaises(validator.ValidationError):
                validator.validate_report(altered)

    def test_p99_claim_must_match_declared_limit(self):
        cases = (
            ("scan_input_to_frame_packet", "display_p99_limit_ms", "display_p99_under_100ms"),
            ("handoff_initiate_accept_take", "handoff_p99_limit_ms", "handoff_p99_under_10ms"),
            ("flight_plan_amendment", "amendment_p99_limit_ms", "amendment_p99_under_10ms"),
        )
        for metric_name, limit_key, check_key in cases:
            with self.subTest(metric=metric_name):
                limit_ns = self.baseline["targets"][limit_key] * 1_000_000
                below = copy.deepcopy(self.baseline)
                below["metrics"][metric_name]["p99"] = limit_ns - 1
                below["metrics"][metric_name]["max"] = limit_ns - 1
                validator.validate_report(below)

                exact = copy.deepcopy(self.baseline)
                exact["metrics"][metric_name]["p99"] = limit_ns
                exact["metrics"][metric_name]["max"] = limit_ns
                with self.assertRaisesRegex(validator.ValidationError, rf"checks\.{check_key} disagrees"):
                    validator.validate_report(exact)

    def test_nan_zero_samples_and_percentile_order_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "nan.json"
            path.write_text('{"schema":"openatca-latency-v1","bad":NaN}', encoding="utf-8")
            result = subprocess.run([sys.executable, str(SCRIPT), str(path)], capture_output=True,
                                    text=True, check=False)
            self.assertNotEqual(result.returncode, 0)
        altered = copy.deepcopy(self.baseline)
        altered["metrics"]["flight_plan_amendment"]["n"] = 0
        with self.assertRaises(validator.ValidationError):
            validator.validate_report(altered)
        altered = copy.deepcopy(self.baseline)
        altered["metrics"]["handoff_initiate_accept_take"]["p95"] = 1
        with self.assertRaises(validator.ValidationError):
            validator.validate_report(altered)


if __name__ == "__main__":
    unittest.main()
