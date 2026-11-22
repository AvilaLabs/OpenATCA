"""Focused source-layout and evidence tests for the allocator acceptance oracle."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location(
    "allocator_validation", Path(__file__).with_name("allocator_validation.py"))
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def result(injected=False):
    return {
        "status": "FAIL_M3_ALLOCATION" if injected else "PASS_M3_EXERCISED",
        "injection_requested": injected,
        "fixture_ids": [
            "tracker_update_ooo_refusal_capacity_advance_flush",
            "tracker_default_4096_capacity_recovery",
            "tracker_output_log_overflow_reuse",
            "tracker_clock_hold_stepback_recovery",
            "fusion_bucket_plot_cap_refusal_recovery",
            "fusion_adsb_pending_queue_capacity_recovery",
            "fusion_plot_pending_queue_capacity_recovery",
            "fusion_adsb_hard_assoc_registration",
            "fusion_anonymous_init_and_assoc",
            "nam_session_activate_traffic_retry_full_deactivate",
            "display_rows_traffic_frame_refusal",
            "display_raw_fallback_draw_capacity_invalid_refusal",
            "safety_state_inhibit_display",
        ],
        "route_ids": [
            'safety_banner_clamp', 'safety_banner_draw', 'safety_build_error',
            'safety_events_take', 'safety_inhibit_create', 'safety_inhibit_draw',
            'safety_inhibit_expire', 'safety_inhibit_refusal', 'safety_inhibit_remove',
            'safety_net_status', 'safety_state_backdated', 'safety_state_future_refusal',
            'safety_state_report', 'safety_suppress_log', 'safety_suppressed_overflow',
            'safety_table_full',
            'fusion_adsb_ingest',
            'fusion_adsb_pending_drain',
            'fusion_adsb_pending_overflow',
            'fusion_adsb_pending_recovery',
            'fusion_advance',
            'fusion_anonymous_initiation',
            'fusion_bucket_capacity_recovery',
            'fusion_bucket_known_match',
            'fusion_bucket_plot_cap',
            'fusion_bucket_unknown_identity',
            'fusion_finish',
            'fusion_local_association',
            'fusion_plot_ingest',
            'fusion_plot_pending_bucket_cap',
            'fusion_plot_pending_overflow',
            'fusion_plot_pending_recovery',
            'fusion_registration',
            'display_rows_to_targets',
            'display_track_end',
            'display_traffic_draw',
            'display_uncertain_radius',
            'display_callsign_conflict',
            'display_altitude_marker',
            'display_set_refusal',
            'display_malformed_target',
            'display_frame_chain',
            'display_map_scene',
            'display_invalid_position',
            'display_unprojectable',
            'display_history_dot_dropped',
            'display_time_overflow',
            'display_unplaced_history_dot',
            'display_stamp_clipped',
            'display_frame_digest',
            'display_frame_refusal',
            'display_raw_frame_draw',
            'display_raw_capacity_refusal',
            'display_raw_invalid_refusal',
            'nam_session_activation',
            'nam_session_traffic',
            'nam_session_resend',
            'nam_session_timeout',
            'nam_session_deliver',
            'nam_session_response',
            'nam_session_keepalive',
            'nam_session_peer_silent',
            'nam_session_pending_full',
            'nam_session_discard',
            'nam_session_abandon',
            'nam_session_deactivation',
            'tracker_advance',
            'tracker_capacity_recovery',
            'tracker_capacity_refusal',
            'tracker_clock_backstep',
            'tracker_clock_forward_recovery',
            'tracker_clock_hold',
            'tracker_clock_recovery',
            'tracker_default_capacity',
            'tracker_default_confirmed_capacity',
            'tracker_flush',
            'tracker_history_retention_refusal',
            'tracker_history_wrap_refilter',
            'tracker_history_wrap_replay',
            'tracker_invalid_input_refusal',
            'tracker_log_overflow',
            'tracker_out_of_order_refilter',
            'tracker_output_overflow',
            'tracker_output_payload_stability',
            'tracker_output_reuse',
            'tracker_prediction',
            'tracker_update',
        ],
        "controls": {"alloc": 2, "zeroed": 1, "realloc": 1, "overflow": False},
        "tracker": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0,
            "overflow": False,
            "applied": 7, "refiltered": 2, "refused": 1, "capacity_refused": 1,
            "full_capacity": 8,
            "recovery_applied": 1,
            "predictions": 4, "advance_rows": 3, "flush_rows": 2, "ended": 1, "rows": 8,
        },
        "default_capacity": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "occupied": 4096, "confirmed": 4096, "refused": 1, "recovered": 1,
            "history_refiltered": 1, "history_replayed": 1, "late_ring": 1,
        },
        "output_log": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "output_overflow": 1, "output_rows_after_reuse": 1, "output_aligned": True,
            "output_preserved_addr": True, "output_payload_unchanged": True,
            "log_overflow": 1, "log_rows": 1,
            "log_aligned": True,
        },
        "clock_gate": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "held": 2, "held_applied": 2, "step_forward": 1, "late": 5,
            "step_back": 1, "future_dropped": 3, "recovered": 1,
        },
        "bucket_capacity": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "plots_offered": 16_386, "bucket_plot_overflow": 1,
            "known_hard_applied": 1, "unmatched_hard_no_track": 16_383,
            "recovery_applied": 1,
        },
        "adsb_pending": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "offered": 1_048_577, "pending_overflow": 1, "drained": 1_048_576,
            "bucket_plot_overflow": 0, "recovery_applied": 2,
            "tracker_applied": 1, "tracker_duplicates": 1_048_575,
            "tracker_refused": 0, "tracker_capacity_refused": 0,
            "tracker_late_window": 0, "tracker_late_ring": 0,
            "tracker_jump_rejected": 0, "tracker_numerical_refused": 0,
            "hard_extra": 0, "hard_mismatch": 0,
        },
        "plot_pending": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "offered": 1_048_577, "pending_overflow": 1, "drained": 1_048_576,
            "bucket_plot_overflow": 1_032_192, "recovery_applied": 1,
            "tracker_applied": 1, "tracker_duplicates": 0,
            "tracker_refused": 0, "tracker_capacity_refused": 0,
            "tracker_late_window": 0, "tracker_late_ring": 0,
            "tracker_jump_rejected": 0, "tracker_numerical_refused": 0,
            "hard_extra": 16_383, "hard_mismatch": 0,
        },
        "fusion": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0,
            "overflow": False,
            "plots": 40, "hard_applied": 2, "candidates_confirmed": 3, "refused": 1,
            "rows": 20, "radar_tracks": 4, "registration_used": 1,
            "registration_history": 3, "anon_applied": 2, "local_rows": 5,
            "init_visits": 3, "init_tests": 2, "coast_rows": 4, "quiet_advance_rows": 3,
        },
        "nam_session": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "irq_sent": 4, "atm_sent": 132, "resent": 2, "timed_out": 2, "delivered": 2,
            "responses": 2, "asm_sent": 1, "peer_silent": 1, "lrm_sent": 1, "trq_sent": 1,
            "abandoned": 128, "peak_pending": 128, "pending_full": 1, "discards": 3,
            "dropped": 0, "script_errors": 0, "final_inactive": True,
        },
        "display_frame": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "convert_added": 1080, "convert_ended": 1300, "traffic_drawn": 64, "uncertain": 631,
            "cs_conflict": 20, "r95_unknown": 269, "refused": 100, "alt_clamped": 4,
            "malformed": 20, "frame_drawn": 2718, "scene_items": 140, "refusing_refused": 2574,
            "convert_invalid_position": 18, "convert_unprojectable": 18,
            "convert_trail_dropped": 63, "convert_time_overflow": 2718,
            "draw_trail_dropped": 20, "stamp_clipped": 5, "digests": 20,
            "raw_drawn": 20, "raw_capacity_refused": 40, "raw_invalid_refused": 20,
        },
        "safety": {
            "alloc": 1 if injected else 0, "zeroed": 0, "realloc": 0, "overflow": False,
            "state_reports": 4, "stale_unavailable": 4, "created": 3, "refused": 3,
            "refused_full": 1, "suppressed_logged": 4096, "suppressed_overflow": 3,
            "removed": 1, "expired": 63, "events": 128, "banner_lines": 160,
            "inhibitions_drawn": 20, "inhibitions_unplaced": 20, "state_backdated": 1,
            "state_future_refused": 1, "banner_hidden": 140, "error_builds": 20,
        },
    }


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def layout_tree(root):
    write(root / "Cargo.toml", '''
[workspace]
members = ["crates/oa-track"]
exclude = ["fuzz", "validation/oa-alloc-check"]
[workspace.lints.rust]
unsafe_code = "forbid"
''')
    write(root / "crates/oa-track/Cargo.toml", '''
[package]
name = "oa-track"
version = "0.1.0"
edition = "2021"
[lints]
workspace = true
''')
    write(root / "validation/oa-alloc-check/Cargo.toml", '''
[package]
name = "oa-alloc-check"
version = "0.1.0"
edition = "2021"
publish = false
[workspace]
[lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"
[dependencies]
oa-track = { path = "../../crates/oa-track" }
oa-nam = { path = "../../crates/oa-nam" }
oa-display = { path = "../../crates/oa-display" }
oa-scope-replay = { path = "../../crates/oa-scope-replay" }
oa-airspace = { path = "../../crates/oa-airspace" }
oa-geo = { path = "../../crates/oa-geo" }
oa-safety = { path = "../../crates/oa-safety" }
''')
    write(root / "validation/oa-alloc-check/src/main.rs", '''
#![deny(unsafe_code)]
fn main() {}
''')
    write(root / "validation/oa-alloc-check/src/allocator.rs", '''
#![allow(unsafe_code)]
#[global_allocator]
static A: System = System;
unsafe impl GlobalAlloc for A {}
''')


class AllocatorResultTests(unittest.TestCase):
    # REQ: OA-TRK-337
    def test_exercised_and_injected_results_pass(self):
        self.assertEqual(gate.evaluate_allocator_result(json.dumps(result())), [])
        self.assertEqual(gate.evaluate_allocator_result(json.dumps(result(True)), True), [])

    def test_injected_mode_needs_alloc_activity_and_matching_status(self):
        value = result(True)
        value["tracker"]["alloc"] = 0
        self.assertTrue(any("tracker.alloc" in e for e in gate.evaluate_allocator_result(json.dumps(value), True)))
        value["tracker"]["alloc"] = 1
        value["status"] = "PASS_M3_EXERCISED"
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value), True))

    def test_missing_empty_wrong_class_and_duplicate_fixtures_fail_closed(self):
        self.assertTrue(gate.evaluate_allocator_result("{}"))
        self.assertTrue(gate.evaluate_allocator_result(""))
        value = result()
        value["fixture_ids"] = []
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        value["fixture_ids"] = ["tracker_update_ooo_refusal_capacity_advance_flush"] * 3
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))

    def test_bool_negative_missing_and_zero_coverage_are_not_integers_or_passes(self):
        value = result()
        value["controls"]["realloc"] = True
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        value = result()
        value["tracker"].pop("rows")
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        value = result()
        value["fusion"]["plots"] = 0
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))

    # REQ: OA-TRK-337
    def test_pending_windows_require_exact_dispositions_for_all_admitted_work(self):
        value = result()
        value["adsb_pending"]["tracker_duplicates"] -= 1
        self.assertTrue(any("adsb_pending.tracker_duplicates" in error
                            for error in gate.evaluate_allocator_result(json.dumps(value))))
        value = result()
        value["plot_pending"]["hard_extra"] = 16_382
        self.assertTrue(any("plot_pending.hard_extra" in error
                            for error in gate.evaluate_allocator_result(json.dumps(value))))

    def test_overflow_partial_json_and_wrong_top_level_fail(self):
        value = result()
        value["overflow"] = True
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(result()) + " trailing"))
        self.assertTrue(gate.evaluate_allocator_result("[]"))
        value = result()
        value["output_log"] = []
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))


class AllocatorLayoutTests(unittest.TestCase):
    # REQ: OA-TRK-337
    def test_standalone_harness_requires_the_strict_runtime_lints(self):
        for lint in ("unwrap_used", "expect_used", "panic", "indexing_slicing"):
            with self.subTest(lint=lint), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                layout_tree(root)
                manifest = root / "validation/oa-alloc-check/Cargo.toml"
                manifest.write_text(manifest.read_text().replace(f'{lint} = "deny"',
                                                                f'{lint} = "allow"'))
                self.assertTrue(any("runtime lints" in error
                                    for error in gate.allocation_layout_errors(root)))

    def test_realistic_isolated_layout_is_accepted(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            self.assertEqual(gate.allocation_layout_errors(root), [])

    # REQ: OA-TRK-337
    def test_route_ledger_requires_source_symbols_and_exact_runtime_route_set(self):
        value = result()
        value["route_ids"] = value["route_ids"][:-1]
        self.assertTrue(any("route_ids" in error for error in
                            gate.evaluate_allocator_result(json.dumps(value))))
        value = result()
        value["tracker"]["recovery_applied"] = 0
        self.assertTrue(any("tracker.recovery_applied" in error for error in
                            gate.evaluate_allocator_result(json.dumps(value))))
        value = result()
        value["default_capacity"]["confirmed"] = 4095
        self.assertTrue(any("default_capacity.confirmed" in error for error in
                            gate.evaluate_allocator_result(json.dumps(value))))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            symbols_by_file = {}
            for route_id, (relative, _) in gate.ROUTE_LEDGER.items():
                symbols_by_file.setdefault(relative, set()).add(gate.ROUTE_SYMBOLS[route_id])
            for relative, symbols in symbols_by_file.items():
                write(root / relative, " ".join(f"fn {symbol}() {{}}" for symbol in sorted(symbols)))
            self.assertEqual(gate.allocation_route_ledger_errors(root), [])
            tracker = root / "crates/oa-track/src/tracker.rs"
            tracker.write_text("fn tracker_predict() {}", encoding="utf-8")
            fusion = root / "crates/oa-track/src/fusion.rs"
            fusion.write_text(fusion.read_text(encoding="utf-8") + " fn tracker_update() {}", encoding="utf-8")
            errors = gate.allocation_route_ledger_errors(root)
            self.assertTrue(any("tracker_update" in error for error in errors))

    def test_workspace_leak_and_harness_dependency_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            manifest = root / "Cargo.toml"
            manifest.write_text(manifest.read_text().replace('"crates/oa-track"', '"crates/oa-track", "validation/*"'), encoding="utf-8")
            self.assertTrue(any("workspace member" in e for e in gate.allocation_layout_errors(root)))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            write(root / "crates/oa-track/Cargo.toml", '''
[package]
name = "oa-track"
version = "0.1.0"
[lints]
workspace = true
[target.'cfg(unix)'.dependencies]
probe = { path = "../../validation/oa-alloc-check" }
''')
            self.assertTrue(any("points back" in e for e in gate.allocation_layout_errors(root)))

    def test_aliased_harness_dependency_and_outside_path_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            manifest = root / "validation/oa-alloc-check/Cargo.toml"
            manifest.write_text(manifest.read_text().replace('oa-track = {', 'track-alias = {'), encoding="utf-8")
            self.assertTrue(any("only its declared local crate" in e for e in gate.allocation_layout_errors(root)))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            external = root.parent / (root.name + "-external")
            # The oracle must identify an escape from the path spelling before it attempts to read it.
            manifest = root / "validation/oa-alloc-check/Cargo.toml"
            manifest.write_text(manifest.read_text().replace("../../crates/oa-track", "../../../" + external.name), encoding="utf-8")
            with patch.object(gate, "_read_toml", wraps=gate._read_toml) as reader:
                self.assertTrue(gate.allocation_layout_errors(root))
                self.assertTrue(all(Path(call.args[0]).resolve().is_relative_to(root.resolve())
                                    for call in reader.call_args_list))

    def test_product_alias_cannot_depend_on_harness(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            manifest = root / "crates/oa-track/Cargo.toml"
            manifest.write_text(manifest.read_text() + '\n[dependencies]\ninnocuous = { package = "oa-alloc-check", path = "../../validation/oa-alloc-check" }\n')
            self.assertTrue(any("allocator harness" in e for e in gate.allocation_layout_errors(root)))

    def test_hashed_raw_strings_cannot_hide_a_local_unsafe_allowance(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            main = root / "validation/oa-alloc-check/src/main.rs"
            main.write_text(
                '#![deny(unsafe_code)]\nmod allocator;\nfn main() {\n'
                'let _ = r#"""#;\n'
                '#[allow(unsafe_code)] unsafe fn marker() { unsafe { std::ptr::read_volatile(&0u8); } }\n'
                '#[allow(unsafe_code)] fn call() { unsafe { marker(); } }\n'
                'let _ = "close"; call();\n}\n')
            self.assertTrue(any("main.rs" in e for e in gate.allocation_layout_errors(root)))

    def test_raw_literal_contents_are_opaque_and_unterminated_literals_fail(self):
        for prefix in ("r", "br", "cr"):
            for hashes in ("", "#", "###"):
                text = f'{prefix}{hashes}"unsafe #[allow(unsafe_code)]"{hashes};'
                self.assertNotIn("unsafe", gate._rust_tokens(text))
        with self.assertRaises(ValueError):
            gate._rust_tokens('r##"never closed')

    def test_safe_pending_fixture_module_is_allowed_but_unsafe_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            helper = root / "validation/oa-alloc-check/src/pending_windows.rs"
            helper.write_text("#![forbid(unsafe_code)]\nfn fixture() {}\n")
            self.assertEqual(gate.allocation_layout_errors(root), [])
            helper.write_text("#![allow(unsafe_code)]\nunsafe fn fixture() {}\n")
            self.assertTrue(any("pending_windows.rs" in e for e in gate.allocation_layout_errors(root)))

    def test_nam_window_module_is_allowed_when_safe_and_rejected_when_unsafe(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            helper = root / "validation/oa-alloc-check/src/nam_window.rs"
            helper.write_text("#![forbid(unsafe_code)]\nfn fixture() {}\n")
            self.assertEqual(gate.allocation_layout_errors(root), [])
            helper.write_text("#![allow(unsafe_code)]\nunsafe fn fixture() {}\n")
            self.assertTrue(any("nam_window.rs" in e for e in gate.allocation_layout_errors(root)))

    def test_nam_session_evidence_must_be_exact(self):
        value = result()
        value["nam_session"]["peak_pending"] = 127
        self.assertTrue(any("nam_session.peak_pending" in e for e in
                            gate.evaluate_allocator_result(json.dumps(value))))
        value = result()
        value["nam_session"]["final_inactive"] = False
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))

    def test_display_window_module_is_allowed_when_safe_and_rejected_when_unsafe(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            helper = root / "validation/oa-alloc-check/src/display_window.rs"
            helper.write_text("#![forbid(unsafe_code)]\nfn fixture() {}\n")
            self.assertEqual(gate.allocation_layout_errors(root), [])
            helper.write_text("#![allow(unsafe_code)]\nunsafe fn fixture() {}\n")
            self.assertTrue(any("display_window.rs" in e for e in gate.allocation_layout_errors(root)))

    def test_display_frame_evidence_must_be_exact_and_exercised(self):
        value = result()
        value["display_frame"]["malformed"] = 0
        self.assertTrue(any("display_frame.malformed" in e for e in
                            gate.evaluate_allocator_result(json.dumps(value))))
        value = result()
        value["display_frame"]["scene_items"] = 0
        self.assertTrue(any("display_frame.scene_items" in e for e in
                            gate.evaluate_allocator_result(json.dumps(value))))
        value = result()
        value["display_frame"]["alloc"] = 1
        self.assertTrue(gate.evaluate_allocator_result(json.dumps(value)))

    def test_repository_allocator_layout_is_valid(self):
        self.assertEqual(gate.allocation_layout_errors(Path(__file__).resolve().parents[1]), [])

    def test_guard_allows_allocator_unsafe_and_rejects_duplicate_attribute(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            layout_tree(root)
            write(root / "validation/oa-alloc-check/src/main.rs", '#![deny(unsafe_code)]\nfn main() { let _s = "unsafe { harmless }"; }\n')
            write(root / "validation/oa-alloc-check/src/allocator.rs", '#![allow(unsafe_code)]\n#[global_allocator]\nstatic A: System = System;\nunsafe impl GlobalAlloc for A { unsafe fn alloc(&self) { unsafe { work(); } } }\n')
            self.assertEqual(gate.allocation_layout_errors(root), [])
            write(root / "validation/oa-alloc-check/src/allocator.rs", '#![allow(unsafe_code)]\n#[global_allocator]\n#[global_allocator]\nstatic A: System = System;\nunsafe impl GlobalAlloc for A {}\n')
            self.assertTrue(any("exactly one global allocator" in e for e in gate.allocation_layout_errors(root)))




class SafetyWindowTests(unittest.TestCase):
    def test_safety_evidence_must_be_exact_and_exercised(self):
        self.assertEqual(gate.evaluate_allocator_result(json.dumps(result())), [])
        for name, bad in (("suppressed_overflow", 2), ("suppressed_logged", 4095),
                          ("error_builds", 0), ("state_future_refused", 0), ("created", 0)):
            value = result()
            value["safety"][name] = bad
            self.assertTrue(any(f"safety.{name}" in e for e in
                                gate.evaluate_allocator_result(json.dumps(value))), name)
        value = result()
        value["safety"]["alloc"] = 1
        self.assertTrue(any("normal safety.alloc" in e for e in
                            gate.evaluate_allocator_result(json.dumps(value))))
        value = result(injected=True)
        value["safety"]["alloc"] = 0
        self.assertTrue(any("injected safety.alloc" in e for e in
                            gate.evaluate_allocator_result(json.dumps(value), expect_injected=True)))


if __name__ == "__main__":
    unittest.main()
